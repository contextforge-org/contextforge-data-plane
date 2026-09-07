use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use cpex::cpex_core::{
    config::CpexConfig,
    factory::{PluginFactory, PluginFactoryRegistry},
};
use rmcp::{ErrorData, model::ErrorCode};
use tokio::task::JoinHandle;

use crate::{
    config::{RedisRuntimePluginConfigStore, RuntimePluginConfigStore, cpex_config},
    error::GatewayPluginRuntimeError,
    hooks::RuntimeHookError,
    runtime::GatewayPluginRuntime,
};

const DEFAULT_CONFIG_WATCHER_INTERVAL: Duration = Duration::from_mins(10);

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
    Active(Arc<GatewayPluginRuntime>),
    Failed(String),
}

impl Default for CpexRuntimeRegistry {
    fn default() -> Self {
        Self {
            runtime: Arc::new(ArcSwap::from_pointee(RuntimeState::Active(Arc::new(GatewayPluginRuntime::default())))),
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
        apply_runtime_config(runtime, factories, Some(cpex_config(&config.document)?)).await?;
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
        drop(runtime.swap(Arc::new(RuntimeState::Active(Arc::new(GatewayPluginRuntime::default())))));
        return Ok(());
    };
    drop(
        runtime.swap(Arc::new(RuntimeState::Active(Arc::new(
            GatewayPluginRuntime::from_config(config, factories).await?,
        )))),
    );
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
    pub(crate) fn current(&self) -> Result<Arc<GatewayPluginRuntime>, ErrorData> {
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
