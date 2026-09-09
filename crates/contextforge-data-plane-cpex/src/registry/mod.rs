use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use contextforge_data_plane_apis::{runtime_plugin_config::RuntimePluginConfigDocument, user_store::ToolPolicyContext};
use cpex::cpex_core::{
    config::CpexConfig,
    factory::{PluginFactory, PluginFactoryRegistry},
};
use rmcp::{ErrorData, model::ErrorCode};
use tokio::task::JoinHandle;

use crate::{
    PluginRequest, PluginRequestContext,
    config::{RedisRuntimePluginConfigStore, RuntimePluginConfigStore},
    error::GatewayPluginRuntimeError,
    hooks::{Operation, RuntimeHookError},
    runtime::GatewayPluginRuntime,
};

const DEFAULT_CONFIG_WATCHER_INTERVAL: Duration = Duration::from_secs(30);

pub struct CpexRuntimeRegistry {
    runtime: Arc<ArcSwap<RuntimeState>>,
    config_store: Option<Arc<dyn RuntimePluginConfigStore>>,
    factories: Arc<PluginFactoryRegistry>,
    watcher_started: AtomicBool,
    watcher_interval: Duration,
}

#[derive(Clone)]
pub struct GatewayPluginRuntimeHandle {
    runtime: Arc<ArcSwap<RuntimeState>>,
}

enum RuntimeState {
    Active(Arc<RuntimePolicies>),
    Failed(String),
}

#[derive(Default)]
pub(crate) struct RuntimePolicies {
    pub(crate) global: Arc<GatewayPluginRuntime>,
    contexts: HashMap<String, Arc<GatewayPluginRuntime>>,
    scoped: bool,
}

impl RuntimePolicies {
    pub(crate) fn resolve(
        &self,
        operation: Operation,
        tool: Option<&ToolPolicyContext>,
    ) -> Result<Arc<GatewayPluginRuntime>, ErrorData> {
        if !matches!(operation, Operation::Tool) || !self.scoped {
            return Ok(Arc::clone(&self.global));
        }
        let tool = tool
            .filter(|tool| !tool.id.is_empty() && !tool.name.is_empty() && !tool.context_id.is_empty())
            .ok_or_else(|| ErrorData::internal_error("Runtime plugin tool context is missing", None))?;
        self.contexts
            .get(&tool.context_id)
            .cloned()
            .ok_or_else(|| ErrorData::internal_error("Runtime plugin policy context is missing", None))
    }
}

impl Default for CpexRuntimeRegistry {
    fn default() -> Self {
        Self {
            runtime: Arc::new(ArcSwap::from_pointee(RuntimeState::Active(Arc::new(RuntimePolicies::default())))),
            config_store: None,
            factories: Arc::new(PluginFactoryRegistry::new()),
            watcher_started: AtomicBool::new(false),
            watcher_interval: DEFAULT_CONFIG_WATCHER_INTERVAL,
        }
    }
}

impl CpexRuntimeRegistry {
    pub fn with_redis_config(redis_client: redis::Client) -> Self {
        Self { config_store: Some(Arc::new(RedisRuntimePluginConfigStore::new(redis_client))), ..Self::default() }
    }

    pub fn register_factory(
        &mut self,
        kind: impl Into<String>,
        factory: Box<dyn PluginFactory>,
    ) -> Result<(), GatewayPluginRuntimeError> {
        let factories = Arc::get_mut(&mut self.factories).ok_or(GatewayPluginRuntimeError::FactoryRegistryShared)?;
        factories.register(kind, factory);
        Ok(())
    }

    pub async fn reload(&self) -> Result<(), GatewayPluginRuntimeError> {
        reload_runtime(&self.runtime, self.config_store.as_ref(), &self.factories, None).await.map(|_| ())
    }

    pub async fn apply_config(&self, config: Option<CpexConfig>) -> Result<(), GatewayPluginRuntimeError> {
        apply_runtime_config(&self.runtime, &self.factories, config).await
    }

    pub async fn apply_document(&self, document: RuntimePluginConfigDocument) -> Result<(), GatewayPluginRuntimeError> {
        apply_document(&self.runtime, &self.factories, document).await
    }

    pub fn handle(&self) -> GatewayPluginRuntimeHandle {
        GatewayPluginRuntimeHandle { runtime: Arc::clone(&self.runtime) }
    }

    fn start_config_watcher(&self, initial_config: Option<Vec<u8>>) -> Option<JoinHandle<()>> {
        let config_store = self.config_store.clone()?;
        if self.watcher_started.swap(true, Ordering::AcqRel) {
            return None;
        }

        let runtime = Arc::downgrade(&self.runtime);
        let factories = Arc::clone(&self.factories);
        let watcher_interval = self.watcher_interval;
        Some(tokio::spawn(async move {
            let mut last_applied_config = initial_config;
            loop {
                tokio::time::sleep(watcher_interval).await;
                let Some(runtime) = runtime.upgrade() else {
                    break;
                };
                match reload_runtime(&runtime, Some(&config_store), &factories, last_applied_config.as_deref()).await {
                    Ok(Some(fingerprint)) => last_applied_config = Some(fingerprint),
                    Ok(None) => {},
                    Err(error) => {
                        tracing::warn!(%error, "failed to reload CPEX runtime plugin config");
                        last_applied_config = None;
                    },
                }
            }
        }))
    }
}

async fn reload_runtime(
    runtime: &ArcSwap<RuntimeState>,
    config_store: Option<&Arc<dyn RuntimePluginConfigStore>>,
    factories: &PluginFactoryRegistry,
    last_applied_config: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, GatewayPluginRuntimeError> {
    let Some(config_store) = config_store else {
        return Ok(None);
    };
    let result = async {
        let config = config_store.get_config().await?.ok_or(GatewayPluginRuntimeError::ConfigMissing)?;
        if last_applied_config == Some(config.fingerprint.as_slice()) {
            return Ok(None);
        }
        apply_document(runtime, factories, config.document).await?;
        Ok(Some(config.fingerprint))
    }
    .await;
    if let Err(error) = &result {
        set_runtime_failed(runtime, error);
    }
    result
}

async fn apply_runtime_config(
    runtime: &ArcSwap<RuntimeState>,
    factories: &PluginFactoryRegistry,
    config: Option<CpexConfig>,
) -> Result<(), GatewayPluginRuntimeError> {
    let Some(config) = config else {
        drop(runtime.swap(Arc::new(RuntimeState::Active(Arc::new(RuntimePolicies::default())))));
        return Ok(());
    };
    drop(runtime.swap(Arc::new(RuntimeState::Active(Arc::new(RuntimePolicies {
        global: Arc::new(GatewayPluginRuntime::from_config(config, factories).await?),
        ..Default::default()
    })))));
    Ok(())
}

async fn apply_document(
    runtime: &ArcSwap<RuntimeState>,
    factories: &PluginFactoryRegistry,
    document: RuntimePluginConfigDocument,
) -> Result<(), GatewayPluginRuntimeError> {
    if !document.enabled {
        return apply_runtime_config(runtime, factories, None).await;
    }
    let global = document.global.ok_or(GatewayPluginRuntimeError::ConfigMissing)?;
    let global = Arc::new(GatewayPluginRuntime::from_published_config(global, &document.settings, factories).await?);
    let mut contexts = HashMap::new();
    for (key, config) in document.contexts {
        if key.is_empty() {
            return Err(GatewayPluginRuntimeError::ConfigWrongFormat);
        }
        let policy = GatewayPluginRuntime::from_published_config(config, &document.settings, factories).await?;
        contexts.insert(key, Arc::new(policy));
    }
    drop(runtime.swap(Arc::new(RuntimeState::Active(Arc::new(RuntimePolicies { global, contexts, scoped: true })))));
    Ok(())
}

fn set_runtime_failed(runtime: &ArcSwap<RuntimeState>, error: &GatewayPluginRuntimeError) {
    drop(runtime.swap(Arc::new(RuntimeState::Failed(error.to_string()))));
}

impl CpexRuntimeRegistry {
    pub async fn initialize(&self) -> Result<Option<JoinHandle<()>>, RuntimeHookError> {
        let initial_config = reload_runtime(&self.runtime, self.config_store.as_ref(), &self.factories, None).await?;
        Ok(self.start_config_watcher(initial_config))
    }
}

impl GatewayPluginRuntimeHandle {
    pub(crate) fn resolve(
        &self,
        operation: Operation,
        context: &PluginRequestContext,
    ) -> Result<(PluginRequest, Arc<GatewayPluginRuntime>), ErrorData> {
        let request = match &context.request {
            Some(request) => request.clone(),
            None => PluginRequest::new(self.current()?, context.extensions.clone()),
        };
        let runtime = request.policies.resolve(operation, context.tool.as_ref())?;
        Ok((request, runtime))
    }

    pub(crate) fn current(&self) -> Result<Arc<RuntimePolicies>, ErrorData> {
        match self.runtime.load().as_ref() {
            RuntimeState::Active(runtime) => Ok(Arc::clone(runtime)),
            RuntimeState::Failed(error) => {
                tracing::warn!(%error, "rejecting MCP call because CPEX runtime is failed");
                Err(ErrorData {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: "Runtime plugin reload failed".into(),
                    data: None,
                })
            },
        }
    }
}

#[cfg(test)]
mod tests;
