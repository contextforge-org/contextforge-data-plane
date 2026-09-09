//! CPEX manager lifecycle and the common pre/post execution path.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use contextforge_data_plane_apis::runtime_plugin_config::RuntimePluginSettings;
use cpex::cpex_core::{
    cmf::{CmfHook, MessagePayload},
    config::CpexConfig,
    executor::PipelineResult,
    factory::PluginFactoryRegistry,
    hooks::{HookTypeDef, payload::Extensions},
    manager::PluginManager,
    plugin::{MatchContext, PluginMode},
    registry::HookEntry,
};
use rmcp::ErrorData;
use tracing::instrument;

use crate::{
    PluginRequest,
    cmf::{CmfResponse, modified_message_payload, plugin_denied_error},
    error::GatewayPluginRuntimeError,
    extension_guard::HeaderWritePolicy,
    factory::supported_hook_name,
    hooks::Operation,
};

#[derive(Default)]
struct HookPair {
    pre: Vec<HookEntry>,
    post: Vec<HookEntry>,
    payload_writes: [bool; 2],
}

#[derive(Default)]
pub(crate) struct GatewayPluginRuntime {
    manager: PluginManager,
    hooks: [HookPair; 4],
    auth_headers_writable: bool,
}

/// Pins operation-specific payload metadata. Mutable CPEX state belongs to the request.
pub(crate) struct CallState {
    runtime: Arc<GatewayPluginRuntime>,
    request: PluginRequest,
    target: Extensions,
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
                if !hook.starts_with("cmf.") && !Operation::Http.hooks().contains(&hook.as_str()) {
                    *hook = format!("cmf.{hook}");
                }
            }
        }
        let mut runtime = Self::from_config(config, factories).await?;
        runtime.auth_headers_writable = settings.plugins_can_override_auth_headers;
        for (hooks, operation) in runtime.hooks.iter_mut().zip(Operation::ALL) {
            hooks.payload_writes = operation.hooks().map(|hook| {
                let name = hook.strip_prefix("cmf.").unwrap_or(hook);
                let field = match name {
                    "tool_pre_invoke" | "prompt_pre_fetch" => "args",
                    "tool_post_invoke" | "prompt_post_fetch" => "result",
                    "resource_pre_fetch" => "uri",
                    "http_pre_request" | "http_post_request" => "headers",
                    _ => "content",
                };
                settings
                    .hook_policies
                    .get(name)
                    .map_or(settings.default_hook_policy == "allow", |policy| policy.writable_fields.contains(field))
            });
        }
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
            HookPair { pre, post, payload_writes: [true; 2] }
        });
        Ok(Self { manager, hooks, auth_headers_writable: false })
    }

    #[instrument(name = "cmf_plugin_before", level = "info", skip_all)]
    pub(crate) async fn before<U: Default>(
        self: &Arc<Self>,
        target: (Operation, &str),
        context: (PluginRequest, Extensions),
        payload: impl FnOnce(&str) -> MessagePayload,
        update: impl FnOnce(&MessagePayload, &str) -> Result<U, ErrorData>,
    ) -> Result<(U, Option<CallState>), ErrorData> {
        let (request, target_extensions) = context;
        let mut extensions = request.extensions().await;
        extensions.meta.clone_from(&target_extensions.meta);
        extensions.mcp.clone_from(&target_extensions.mcp);
        let (operation, name) = target;
        let hooks = &self.hooks[operation as usize];
        let pre = matching_entries(&hooks.pre, &extensions);
        let post = matching_entries(&hooks.post, &extensions);
        if pre.is_empty() && post.is_empty() {
            return Ok((U::default(), None));
        }

        let id = format!("{}-{}", operation.id_prefix(), CORRELATION_ID.fetch_add(1, Ordering::Relaxed));
        let update = if pre.is_empty() {
            U::default()
        } else {
            let result = self.invoke::<CmfHook>((operation, 0, &pre), payload(&id), &request, &target_extensions).await;
            if result.is_denied() {
                return Err(plugin_denied_error(operation.subject(), result));
            }
            match modified_message_payload(&result) {
                Some(payload) => update(payload, &id)?,
                None => U::default(),
            }
        };
        let state = (!post.is_empty()).then(|| CallState {
            runtime: Arc::clone(self),
            request,
            target: target_extensions,
            post,
            name: name.to_owned(),
            id,
        });
        Ok((update, state))
    }

    pub(crate) fn entries(&self, operation: Operation, phase: usize, extensions: &Extensions) -> Vec<HookEntry> {
        let hooks = &self.hooks[operation as usize];
        matching_entries(if phase == 0 { &hooks.pre } else { &hooks.post }, extensions)
    }

    #[instrument(name = "gateway_plugin_invoke", level = "info", skip_all, fields(hook = invocation.0.hooks()[invocation.1]))]
    pub(crate) async fn invoke<H: HookTypeDef>(
        &self,
        invocation: (Operation, usize, &[HookEntry]),
        payload: H::Payload,
        request: &PluginRequest,
        target: &Extensions,
    ) -> PipelineResult {
        let (operation, phase, entries) = invocation;
        let hook = operation.hooks()[phase];
        // Only this request is serialized. No registry or gateway lock crosses plugin I/O.
        let mut state = request.state.lock().await;
        state.extensions.meta.clone_from(&target.meta);
        state.extensions.mcp.clone_from(&target.mcp);
        state.prepare(entries);
        let writable = self.hooks[operation as usize].payload_writes[phase];
        let http = matches!(operation, Operation::Http);
        let header_policy = if http && !writable {
            HeaderWritePolicy::ReadOnly
        } else if self.auth_headers_writable {
            HeaderWritePolicy::All
        } else if http && phase == 0 {
            HeaderWritePolicy::AllowNewCredentials
        } else {
            HeaderWritePolicy::PreserveCredentials
        };
        let guarded =
            crate::extension_guard::guarded_entries(entries, &state.extensions, !http && writable, header_policy);
        let (result, background_tasks) = self
            .manager
            .invoke_entries::<H>(&guarded, payload, state.extensions.clone(), Some(state.contexts.clone()))
            .await;
        for error in &result.errors {
            tracing::warn!(
                hook,
                plugin = error.plugin_name,
                code = error.code.as_deref().unwrap_or(""),
                proto_error_code = error.proto_error_code,
                "CPEX plugin soft error"
            );
        }
        if !result.is_denied() {
            state.contexts = result.context_table.clone();
            if let Some(extensions) = &result.modified_extensions {
                state.extensions = extensions.clone();
            }
        }
        drop(background_tasks);
        result
    }
}

impl CallState {
    pub(crate) async fn after<T: CmfResponse>(&self, response: T) -> Result<T, ErrorData> {
        let result = self.invoke(&response).await?;
        if result.is_denied() {
            return Err(plugin_denied_error(T::OPERATION.subject(), result));
        }
        self.apply(response, &result)
    }

    pub(crate) async fn invoke<T: CmfResponse>(&self, response: &T) -> Result<PipelineResult, ErrorData> {
        let payload = response.to_payload(&self.name, &self.id)?;
        let result =
            self.runtime.invoke::<CmfHook>((T::OPERATION, 1, &self.post), payload, &self.request, &self.target).await;
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
        if plugin.hooks.iter().any(|hook| supported_hook_name(hook).is_none()) {
            return Err(GatewayPluginRuntimeError::ConfigUnsupported);
        }
    }

    Ok(())
}
