//! CPEX manager lifecycle and the common pre/post execution path.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use cpex::cpex_core::{
    cmf::{CmfHook, MessagePayload},
    config::CpexConfig,
    context::PluginContextTable,
    executor::PipelineResult,
    factory::PluginFactoryRegistry,
    hooks::payload::Extensions,
    manager::PluginManager,
};
use rmcp::ErrorData;
use tracing::instrument;

use crate::{
    cmf::{CmfResponse, Operation, modified_message_payload, plugin_denied_error},
    error::GatewayPluginRuntimeError,
    factory::supported_cmf_hook_name,
};

#[derive(Default)]
struct HookPair {
    pre: bool,
    post: bool,
}

#[derive(Default)]
pub(crate) struct GatewayPluginRuntime {
    manager: PluginManager,
    hooks: [HookPair; 3],
}

/// Pins the selected runtime and correlation context until the request finishes.
/// Only tool calls wrap this in a mutex, because events also update their context.
pub(crate) struct CallState {
    runtime: Arc<GatewayPluginRuntime>,
    context_table: PluginContextTable,
    name: String,
    id: String,
}

static CORRELATION_ID: AtomicU64 = AtomicU64::new(1);

impl GatewayPluginRuntime {
    pub(crate) async fn from_config(
        config: CpexConfig,
        factories: &PluginFactoryRegistry,
    ) -> Result<Self, GatewayPluginRuntimeError> {
        validate_gateway_supported_config(&config)?;

        let hooks = Operation::ALL.map(|operation| {
            let [pre, post] = operation.hooks().map(|name| declares(&config, name));
            HookPair { pre, post }
        });
        let manager = PluginManager::from_config(config, factories)
            .map_err(|source| GatewayPluginRuntimeError::Configuration { hook: "config", source })?;
        manager.initialize().await.map_err(|source| GatewayPluginRuntimeError::Initialization { source })?;
        Ok(Self { manager, hooks })
    }

    #[instrument(name = "cmf_plugin_before", level = "info", skip(self, payload, update))]
    pub(crate) async fn before<U: Default>(
        self: &Arc<Self>,
        operation: Operation,
        name: &str,
        payload: impl FnOnce(&str) -> MessagePayload,
        update: impl FnOnce(&MessagePayload, &str) -> Result<U, ErrorData>,
    ) -> Result<(U, Option<CallState>), ErrorData> {
        let hooks = &self.hooks[operation as usize];
        if !hooks.pre && !hooks.post {
            return Ok((U::default(), None));
        }

        let id = format!("{}-{}", operation.id_prefix(), CORRELATION_ID.fetch_add(1, Ordering::Relaxed));
        let (update, context_table) = if hooks.pre {
            let result = self.invoke(operation.hooks()[0], payload(&id), None).await;
            if result.is_denied() {
                return Err(plugin_denied_error(operation.subject(), result));
            }
            let update = match modified_message_payload(&result) {
                Some(payload) => update(payload, &id)?,
                None => U::default(),
            };
            (update, result.context_table)
        } else {
            (U::default(), PluginContextTable::default())
        };
        let state =
            hooks.post.then(|| CallState { runtime: Arc::clone(self), context_table, name: name.to_owned(), id });
        Ok((update, state))
    }

    #[instrument(name = "cmf_plugin_invoke", level = "info", skip(self, payload, context_table))]
    async fn invoke(
        &self,
        hook: &'static str,
        payload: MessagePayload,
        context_table: Option<PluginContextTable>,
    ) -> PipelineResult {
        let (result, background_tasks) =
            self.manager.invoke_named::<CmfHook>(hook, payload, Extensions::default(), context_table).await;
        for error in &result.errors {
            tracing::warn!(
                hook,
                plugin = error.plugin_name,
                code = error.code.as_deref().unwrap_or(""),
                proto_error_code = error.proto_error_code,
                "CPEX plugin soft error"
            );
        }
        drop(background_tasks);
        result
    }
}

impl CallState {
    pub(crate) async fn after<T: CmfResponse>(&mut self, response: T) -> Result<T, ErrorData> {
        let result = self.invoke(&response).await?;
        if result.is_denied() {
            return Err(plugin_denied_error(T::OPERATION.subject(), result));
        }
        self.apply(response, &result)
    }

    pub(crate) async fn invoke<T: CmfResponse>(&mut self, response: &T) -> Result<PipelineResult, ErrorData> {
        let payload = response.to_payload(&self.name, &self.id)?;
        let result = self.runtime.invoke(T::OPERATION.hooks()[1], payload, Some(self.context_table.clone())).await;
        if !result.is_denied() {
            self.context_table = result.context_table.clone();
        }
        Ok(result)
    }

    pub(crate) fn apply<T: CmfResponse>(&self, response: T, result: &PipelineResult) -> Result<T, ErrorData> {
        match modified_message_payload(result) {
            Some(payload) => response.apply_payload(payload, &self.name, &self.id),
            None => Ok(response),
        }
    }
}

impl Drop for GatewayPluginRuntime {
    fn drop(&mut self) {
        let manager = std::mem::take(&mut self.manager);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    manager.shutdown().await;
                });
            },
            Err(error) => tracing::warn!(%error, "skipping CPEX plugin shutdown outside a Tokio runtime"),
        }
    }
}

fn declares(config: &CpexConfig, hook_name: &str) -> bool {
    config.plugins.iter().any(|plugin| plugin.hooks.iter().any(|hook| hook == hook_name))
}

fn validate_gateway_supported_config(config: &CpexConfig) -> Result<(), GatewayPluginRuntimeError> {
    if config.routing_enabled()
        || config.plugin_settings.fail_on_plugin_error
        || !config.routes.is_empty()
        || !config.plugin_dirs.is_empty()
        || !config.global.policies.is_empty()
        || !config.global.defaults.is_empty()
    {
        return Err(GatewayPluginRuntimeError::ConfigUnsupported);
    }

    for plugin in &config.plugins {
        if !plugin.conditions.is_empty() {
            return Err(GatewayPluginRuntimeError::ConfigUnsupported);
        }

        if plugin.hooks.iter().any(|hook| supported_cmf_hook_name(hook).is_none()) {
            return Err(GatewayPluginRuntimeError::ConfigUnsupported);
        }
    }

    Ok(())
}
