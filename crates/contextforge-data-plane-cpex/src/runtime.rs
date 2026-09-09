//! CPEX manager lifecycle and the common pre/post execution path.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use contextforge_data_plane_apis::runtime_plugin_config::RuntimePluginSettings;
use cpex::cpex_core::{
    cmf::{CmfHook, MessagePayload},
    config::CpexConfig,
    context::PluginContextTable,
    executor::PipelineResult,
    factory::PluginFactoryRegistry,
    hooks::payload::Extensions,
    manager::PluginManager,
    plugin::{MatchContext, PluginMode},
    registry::HookEntry,
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
    pre: Vec<HookEntry>,
    post: Vec<HookEntry>,
}

#[derive(Default)]
pub(crate) struct GatewayPluginRuntime {
    manager: PluginManager,
    hooks: [HookPair; 3],
    payload_writes: [[bool; 2]; 3],
    auth_headers_writable: bool,
}

/// Pins the selected runtime and correlation context until the request finishes.
/// Only tool calls wrap this in a mutex, because events also update their context.
pub(crate) struct CallState {
    runtime: Arc<GatewayPluginRuntime>,
    context_table: PluginContextTable,
    extensions: Extensions,
    post: Vec<HookEntry>,
    name: String,
    id: String,
}

static CORRELATION_ID: AtomicU64 = AtomicU64::new(1);

impl GatewayPluginRuntime {
    pub(crate) async fn from_published_config(
        mut config: CpexConfig,
        settings: &RuntimePluginSettings,
        factories: &PluginFactoryRegistry,
    ) -> Result<Self, GatewayPluginRuntimeError> {
        if !matches!(settings.default_hook_policy.as_str(), "allow" | "deny") {
            return Err(GatewayPluginRuntimeError::ConfigWrongFormat);
        }
        config.plugin_settings.plugin_timeout = settings.plugin_timeout;
        config.plugins.retain(|plugin| plugin.mode != PluginMode::Disabled);
        for plugin in &mut config.plugins {
            for hook in &mut plugin.hooks {
                // Python publishes native hook names; registered Rust handlers use CMF.
                if !hook.starts_with("cmf.") {
                    *hook = format!("cmf.{hook}");
                }
            }
        }
        let mut runtime = Self::from_config(config, factories).await?;
        runtime.auth_headers_writable = settings.plugins_can_override_auth_headers;
        runtime.payload_writes = Operation::ALL.map(|operation| {
            operation.hooks().map(|hook| {
                let name = hook.strip_prefix("cmf.").unwrap_or(hook);
                let field = match name {
                    "tool_pre_invoke" | "prompt_pre_fetch" => "args",
                    "tool_post_invoke" | "prompt_post_fetch" => "result",
                    "resource_pre_fetch" => "uri",
                    _ => "content",
                };
                settings
                    .hook_policies
                    .get(name)
                    .map_or(settings.default_hook_policy == "allow", |policy| policy.writable_fields.contains(field))
            })
        });
        Ok(runtime)
    }

    pub(crate) async fn from_config(
        config: CpexConfig,
        factories: &PluginFactoryRegistry,
    ) -> Result<Self, GatewayPluginRuntimeError> {
        validate_gateway_supported_config(&config)?;

        let names = config.plugins.iter().map(|plugin| plugin.name.clone()).collect::<Vec<_>>();
        let manager = PluginManager::from_config(config, factories)
            .map_err(|source| GatewayPluginRuntimeError::Configuration { hook: "config", source })?;
        manager.initialize().await.map_err(|source| GatewayPluginRuntimeError::Initialization { source })?;
        let mut entries = names.iter().flat_map(|name| manager.find_plugin_entries(name)).collect::<Vec<_>>();
        // invoke_entries expects priority order; preserve config order for ties.
        entries.sort_by_key(|(_, entry)| entry.plugin_ref.priority());
        let hooks = Operation::ALL.map(|operation| {
            let [pre, post] = operation
                .hooks()
                .map(|name| entries.iter().filter(|(hook, _)| hook == name).map(|(_, entry)| entry.clone()).collect());
            HookPair { pre, post }
        });
        Ok(Self { manager, hooks, payload_writes: [[true; 2]; 3], auth_headers_writable: false })
    }

    #[instrument(name = "cmf_plugin_before", level = "info", skip(self, target, payload, update, extensions))]
    pub(crate) async fn before<U: Default>(
        self: &Arc<Self>,
        target: (Operation, &str),
        mut extensions: Extensions,
        payload: impl FnOnce(&str) -> MessagePayload,
        update: impl FnOnce(&MessagePayload, &str) -> Result<U, ErrorData>,
    ) -> Result<(U, Option<CallState>), ErrorData> {
        let (operation, name) = target;
        let hooks = &self.hooks[operation as usize];
        let pre = matching_entries(&hooks.pre, &extensions);
        let post = matching_entries(&hooks.post, &extensions);
        if pre.is_empty() && post.is_empty() {
            return Ok((U::default(), None));
        }

        let id = format!("{}-{}", operation.id_prefix(), CORRELATION_ID.fetch_add(1, Ordering::Relaxed));
        let (update, context_table) = if pre.is_empty() {
            (U::default(), PluginContextTable::default())
        } else {
            let result = self.invoke((operation.hooks()[0], &pre), payload(&id), extensions.clone(), None).await;
            if result.is_denied() {
                return Err(plugin_denied_error(operation.subject(), result));
            }
            let update = match modified_message_payload(&result) {
                Some(payload) => update(payload, &id)?,
                None => U::default(),
            };
            if let Some(modified) = result.modified_extensions {
                extensions = modified;
            }
            (update, result.context_table)
        };
        let state = (!post.is_empty()).then(|| CallState {
            runtime: Arc::clone(self),
            context_table,
            extensions,
            post,
            name: name.to_owned(),
            id,
        });
        Ok((update, state))
    }

    #[instrument(name = "cmf_plugin_invoke", level = "info", skip_all)]
    async fn invoke(
        &self,
        invocation: (&'static str, &[HookEntry]),
        payload: MessagePayload,
        extensions: Extensions,
        context_table: Option<PluginContextTable>,
    ) -> PipelineResult {
        let (hook, entries) = invocation;
        let payload_writable = Operation::ALL
            .iter()
            .enumerate()
            .find_map(|(i, operation)| {
                operation.hooks().iter().position(|name| *name == hook).map(|j| self.payload_writes[i][j])
            })
            .unwrap_or(false);
        let entries =
            crate::extension_guard::guarded_entries(entries, &extensions, payload_writable, self.auth_headers_writable);
        let (result, background_tasks) =
            self.manager.invoke_entries::<CmfHook>(&entries, payload, extensions, context_table).await;
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
        let result = self
            .runtime
            .invoke(
                (T::OPERATION.hooks()[1], &self.post),
                payload,
                self.extensions.clone(),
                Some(self.context_table.clone()),
            )
            .await;
        if !result.is_denied() {
            self.context_table = result.context_table.clone();
            if let Some(extensions) = &result.modified_extensions {
                self.extensions = extensions.clone();
            }
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

fn matching_entries(entries: &[HookEntry], extensions: &Extensions) -> Vec<HookEntry> {
    let meta = extensions.meta.as_deref();
    let name = meta.and_then(|meta| meta.entity_name.as_deref());
    let kind = meta.and_then(|meta| meta.entity_type.as_deref());
    let mcp = extensions.mcp.as_deref();
    let context = MatchContext {
        server_id: mcp.and_then(|mcp| {
            mcp.tool
                .as_ref()
                .and_then(|tool| tool.server_id.as_deref())
                .or_else(|| mcp.prompt.as_ref().and_then(|prompt| prompt.server_id.as_deref()))
                .or_else(|| mcp.resource.as_ref().and_then(|resource| resource.server_id.as_deref()))
        }),
        tenant_id: meta.and_then(|meta| meta.scope.as_deref()),
        tool: (kind == Some("tool")).then_some(name).flatten(),
        prompt: (kind == Some("prompt")).then_some(name).flatten(),
        resource: (kind == Some("resource")).then_some(name).flatten(),
        user: extensions
            .security
            .as_ref()
            .and_then(|security| security.subject.as_ref())
            .and_then(|subject| subject.id.as_deref()),
        content_type: extensions.http.as_ref().and_then(|http| http.get_request_header("content-type")),
        agent: extensions.agent.as_ref().and_then(|agent| agent.agent_id.as_deref()),
    };
    entries
        .iter()
        .filter(|entry| {
            let config = entry.plugin_ref.trusted_config();
            config.conditions.is_empty() || config.conditions.iter().any(|condition| condition.matches(&context))
        })
        .cloned()
        .collect()
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
        if plugin.hooks.iter().any(|hook| supported_cmf_hook_name(hook).is_none()) {
            return Err(GatewayPluginRuntimeError::ConfigUnsupported);
        }
    }

    Ok(())
}
