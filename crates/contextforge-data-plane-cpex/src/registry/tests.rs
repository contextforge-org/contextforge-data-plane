use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use cpex::cpex_core::{
    cmf::{CmfHook, ContentPart, MessagePayload},
    context::PluginContext,
    error::{PluginError, PluginViolation},
    factory::{PluginFactory, PluginInstance},
    hooks::{Extensions, HookHandler, PluginResult, TypedHandlerAdapter, types::cmf_hook_names},
    plugin::{Plugin, PluginConfig},
    registry::AnyHookHandler,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, NumberOrString, ProgressNotificationParam, ProgressToken,
    ReadResourceResult, ResourceContents,
};
use serde_json::{Value, json};
use tokio::sync::Mutex as TokioMutex;

use contextforge_data_plane_apis::runtime_plugin_config::{RuntimePluginConfigDocument, RuntimePluginSettings};

use crate::config::LoadedRuntimePluginConfig;
use crate::{ArgumentsUpdate, CmfPluginFactory, PreHookResult, ToolHookState};
use rmcp::model::GetPromptRequestParams;

use super::*;

const TEST_MISSING_CONTEXT_ERROR_CODE: i64 = -32003;
const TEST_REWRITTEN_SUM_A: i64 = 10;
const TEST_REWRITTEN_SUM_B: i64 = 20;
const TEST_REWRITTEN_PROMPT_TOPIC: &str = "rewritten-topic";
const TEST_SHUTDOWN_RETRY_COUNT: usize = 20;
const TEST_SHUTDOWN_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const TEST_WATCHER_INTERVAL: Duration = Duration::from_millis(10);
const TEST_WATCHER_RETRY_COUNT: usize = 20;
const TEST_WATCHER_RETRY_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Default)]
struct MemoryConfigStore {
    config: Arc<TokioMutex<Option<RuntimePluginConfigDocument>>>,
    calls: Arc<AtomicUsize>,
}

impl MemoryConfigStore {
    fn with_config(config: RuntimePluginConfigDocument) -> Self {
        Self { config: Arc::new(TokioMutex::new(Some(config))), calls: Arc::new(AtomicUsize::new(0)) }
    }

    async fn set_config(&self, config: RuntimePluginConfigDocument) {
        *self.config.lock().await = Some(config);
    }

    async fn clear_config(&self) {
        *self.config.lock().await = None;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RuntimePluginConfigStore for MemoryConfigStore {
    async fn get_config(&self) -> Result<Option<LoadedRuntimePluginConfig>, GatewayPluginRuntimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.config.lock().await.as_ref().map(loaded_config))
    }
}

#[derive(Default)]
struct Observations {
    pre_calls: usize,
    post_calls: usize,
    shutdown_calls: usize,
    pre_request_id: Option<String>,
    post_request_id: Option<String>,
}

#[derive(Clone, Copy, Default)]
enum PreBehavior {
    #[default]
    Allow,
    Rewrite,
    SetContext,
}

#[derive(Clone, Copy, Default)]
enum PostBehavior {
    #[default]
    Allow,
    Rewrite,
    RewriteStreamEvent,
    RewriteInvalid,
    Deny,
    RequireContext,
    CountEvents,
}

struct TestPlugin {
    config: PluginConfig,
    observations: Arc<Mutex<Observations>>,
    pre_behavior: PreBehavior,
    post_behavior: PostBehavior,
}

impl TestPlugin {
    fn new(name: &str, hooks: Vec<&'static str>) -> Self {
        Self {
            config: PluginConfig {
                name: name.to_owned(),
                kind: "test".to_owned(),
                hooks: hooks.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            },
            observations: Arc::new(Mutex::new(Observations::default())),
            pre_behavior: PreBehavior::Allow,
            post_behavior: PostBehavior::Allow,
        }
    }

    fn rewrite_from_config(config: PluginConfig) -> Self {
        Self { config, ..Self::new("generic-pre", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite() }
    }

    fn with_pre_rewrite(mut self) -> Self {
        self.pre_behavior = PreBehavior::Rewrite;
        self
    }

    fn with_post_rewrite(mut self) -> Self {
        self.post_behavior = PostBehavior::Rewrite;
        self
    }

    fn with_stream_event_rewrite(mut self) -> Self {
        self.post_behavior = PostBehavior::RewriteStreamEvent;
        self
    }

    fn with_invalid_stream_rewrite(mut self) -> Self {
        self.post_behavior = PostBehavior::RewriteInvalid;
        self
    }

    fn with_post_deny(mut self) -> Self {
        self.post_behavior = PostBehavior::Deny;
        self
    }

    fn with_context_roundtrip(mut self) -> Self {
        self.pre_behavior = PreBehavior::SetContext;
        self.post_behavior = PostBehavior::RequireContext;
        self
    }

    fn observations(&self) -> Arc<Mutex<Observations>> {
        Arc::clone(&self.observations)
    }
}

#[async_trait]
impl Plugin for TestPlugin {
    fn config(&self) -> &PluginConfig {
        &self.config
    }

    async fn shutdown(&self) -> Result<(), Box<PluginError>> {
        self.observations.lock().expect("observations lock poisoned").shutdown_calls += 1;
        Ok(())
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl HookHandler<CmfHook> for TestPlugin {
    async fn handle(
        &self,
        payload: &MessagePayload,
        _extensions: &Extensions,
        ctx: &mut PluginContext,
    ) -> PluginResult<MessagePayload> {
        let is_post = payload.message.content.iter().any(|part| {
            matches!(
                part,
                ContentPart::ToolResult { .. } | ContentPart::PromptResult { .. } | ContentPart::Resource { .. }
            )
        });
        let mut observations = self.observations.lock().expect("observations lock poisoned");
        if is_post {
            observations.post_calls += 1;
            observations.post_request_id = request_id(payload);
        } else {
            observations.pre_calls += 1;
            observations.pre_request_id = request_id(payload);
        }
        drop(observations);

        if is_post {
            match self.post_behavior {
                PostBehavior::Allow => PluginResult::allow(),
                PostBehavior::Rewrite => PluginResult::modify_payload(payload.clone()),
                PostBehavior::RewriteStreamEvent => {
                    let mut modified = payload.clone();
                    if let Some(ContentPart::ToolResult { content }) =
                        modified.message.content.iter_mut().find(|part| matches!(part, ContentPart::ToolResult { .. }))
                        && let Ok(mut progress) =
                            serde_json::from_value::<ProgressNotificationParam>(content.content.clone())
                    {
                        progress.message = progress.message.map(|message| format!("plugin:{message}"));
                        content.content = serde_json::to_value(progress).expect("progress serializes");
                    }
                    PluginResult::modify_payload(modified)
                },
                PostBehavior::RewriteInvalid => {
                    let mut modified = payload.clone();
                    if let Some(ContentPart::ToolResult { content }) =
                        modified.message.content.iter_mut().find(|part| matches!(part, ContentPart::ToolResult { .. }))
                    {
                        content.content = json!("not-a-stream-event");
                    }
                    PluginResult::modify_payload(modified)
                },
                PostBehavior::Deny => PluginResult::deny(PluginViolation::new("post_denied", "post denied")),
                PostBehavior::CountEvents => {
                    let results = payload.message.get_tool_results();
                    let result = results.first().expect("tool result");
                    let events = ctx.get_global("events").and_then(Value::as_u64).unwrap_or_default();
                    if result.content.get("progress").is_some() {
                        ctx.set_global("events", json!(events + 1));
                        PluginResult::allow()
                    } else if events == 2 {
                        PluginResult::allow()
                    } else {
                        PluginResult::deny(PluginViolation::new("missing_events", "tool event context was lost"))
                    }
                },
                PostBehavior::RequireContext => {
                    if ctx.get_global("pre_seen") == Some(&json!(true)) {
                        PluginResult::allow()
                    } else {
                        PluginResult::deny(
                            PluginViolation::new("missing_context", "pre context missing")
                                .with_proto_error_code(TEST_MISSING_CONTEXT_ERROR_CODE),
                        )
                    }
                },
            }
        } else {
            match self.pre_behavior {
                PreBehavior::Allow => PluginResult::allow(),
                PreBehavior::Rewrite => {
                    let mut modified = payload.clone();
                    if let Some(ContentPart::ToolCall { content }) =
                        modified.message.content.iter_mut().find(|part| matches!(part, ContentPart::ToolCall { .. }))
                    {
                        content.arguments = HashMap::from([
                            ("a".to_owned(), json!(TEST_REWRITTEN_SUM_A)),
                            ("b".to_owned(), json!(TEST_REWRITTEN_SUM_B)),
                        ]);
                    }
                    if let Some(ContentPart::PromptRequest { content }) = modified
                        .message
                        .content
                        .iter_mut()
                        .find(|part| matches!(part, ContentPart::PromptRequest { .. }))
                    {
                        content.arguments = HashMap::from([("topic".to_owned(), json!(TEST_REWRITTEN_PROMPT_TOPIC))]);
                    }
                    PluginResult::modify_payload(modified)
                },
                PreBehavior::SetContext => {
                    ctx.set_global("pre_seen", json!(true));
                    PluginResult::allow()
                },
            }
        }
    }
}

struct TestPluginFactory {
    observations: Arc<Mutex<Observations>>,
    pre_behavior: PreBehavior,
    post_behavior: PostBehavior,
}

impl TestPluginFactory {
    fn from_plugin(plugin: &TestPlugin) -> Self {
        Self {
            observations: Arc::clone(&plugin.observations),
            pre_behavior: plugin.pre_behavior,
            post_behavior: plugin.post_behavior,
        }
    }
}

impl PluginFactory for TestPluginFactory {
    fn create(&self, config: &PluginConfig) -> Result<PluginInstance, Box<PluginError>> {
        let plugin = Arc::new(TestPlugin {
            config: config.clone(),
            observations: Arc::clone(&self.observations),
            pre_behavior: self.pre_behavior,
            post_behavior: self.post_behavior,
        });
        let handlers = config
            .hooks
            .iter()
            .filter_map(|hook| {
                let hook = crate::factory::supported_cmf_hook_name(hook)?;
                Some((
                    hook,
                    Arc::new(TypedHandlerAdapter::<CmfHook, _>::new(Arc::clone(&plugin))) as Arc<dyn AnyHookHandler>,
                ))
            })
            .collect();
        let plugin: Arc<dyn Plugin> = plugin;
        Ok(PluginInstance { plugin, handlers })
    }
}

fn sum_request(a: i64, b: i64) -> CallToolRequestParams {
    CallToolRequestParams::new("sum")
        .with_arguments(serde_json::Map::from_iter([("a".to_owned(), json!(a)), ("b".to_owned(), json!(b))]))
}

fn review_request(topic: &str) -> GetPromptRequestParams {
    GetPromptRequestParams::new("review")
        .with_arguments(serde_json::Map::from_iter([("topic".to_owned(), json!(topic))]))
}

fn progress_event() -> ProgressNotificationParam {
    ProgressNotificationParam::new(ProgressToken(NumberOrString::String("stream-token".into())), 1.0)
        .with_message("step 1/2")
}

fn config_document(cpex: Value) -> RuntimePluginConfigDocument {
    let config: CpexConfig = serde_json::from_value(cpex).expect("test CPEX config parses");
    let mut global = config.clone();
    let mut scoped = config;
    for plugin in &mut global.plugins {
        plugin.hooks.retain(|hook| !hook.contains("tool_"));
    }
    global.plugins.retain(|plugin| !plugin.hooks.is_empty());
    for plugin in &mut scoped.plugins {
        plugin.hooks.retain(|hook| hook.contains("tool_"));
    }
    scoped.plugins.retain(|plugin| !plugin.hooks.is_empty());
    RuntimePluginConfigDocument {
        enabled: true,
        global: Some(global),
        contexts: HashMap::from([("test".to_owned(), scoped)]),
        settings: RuntimePluginSettings::default(),
    }
}

fn loaded_config(document: &RuntimePluginConfigDocument) -> LoadedRuntimePluginConfig {
    LoadedRuntimePluginConfig::decode(serde_json::to_vec(document).expect("test CPEX config serializes"))
        .expect("test CPEX config decodes")
}

fn plugin_config(plugins: &[Arc<TestPlugin>]) -> RuntimePluginConfigDocument {
    config_document(json!({
        "plugins": plugins.iter().map(|plugin| {
            json!({
                "name": plugin.config.name.clone(),
                "kind": plugin.config.kind.clone(),
                "hooks": plugin.config.hooks.clone(),
            })
        }).collect::<Vec<_>>()
    }))
}

fn expect_runtime_failed(result: Result<PreHookResult<ToolHookState>, ErrorData>) -> ErrorData {
    match result {
        Ok(_) => panic!("runtime should be failed"),
        Err(error) => error,
    }
}

async fn runtime_with_plugin(plugin: &Arc<TestPlugin>, config: RuntimePluginConfigDocument) -> CpexRuntimeRegistry {
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config)));
    runtime.register_factory("test", Box::new(TestPluginFactory::from_plugin(plugin))).expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");
    runtime
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn runtime_config_store_is_loaded_on_initialize() {
    let config_store = MemoryConfigStore::with_config(config_document(json!({ "plugins": [] })));
    let runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));

    let handle = runtime.initialize().await.expect("runtime initializes");

    assert!(handle.is_some());
    assert!(config_store.calls() >= 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn missing_runtime_plugin_config_is_rejected_on_initialize() {
    let runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::default()));

    let error = runtime.initialize().await.expect_err("missing config is rejected");

    assert_eq!("runtime plugin config is missing", error.to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn invalid_runtime_plugin_config_documents_are_rejected() {
    for config in [RuntimePluginConfigDocument {
        enabled: true,
        global: None,
        contexts: HashMap::new(),
        settings: RuntimePluginSettings::default(),
    }] {
        let runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config)));
        let error = runtime.initialize().await.expect_err("invalid config is rejected");

        assert_eq!("runtime plugin config is missing", error.to_string());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn unsupported_runtime_plugin_config_is_rejected() {
    for cpex in [
        json!({ "plugin_settings": { "routing_enabled": true }, "plugins": [] }),
        json!({ "plugin_settings": { "fail_on_plugin_error": true }, "plugins": [] }),
        json!({ "plugins": [{ "name": "llm", "kind": "test", "hooks": [cmf_hook_names::LLM_INPUT] }] }),
    ] {
        let runtime =
            CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config_document(cpex))));
        let error = runtime.initialize().await.expect_err("unsupported config is rejected");

        assert_eq!("runtime plugin config is unsupported", error.to_string());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn prompt_hooks_are_accepted_config() {
    let plugin = Arc::new(TestPlugin::new("prompt", vec![cmf_hook_names::PROMPT_PRE_FETCH]));
    // runtime_with_plugin initializes and expects success
    runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn resource_pre_hook_runs_for_a_canonical_uri() {
    let plugin = Arc::new(TestPlugin::new("resource", vec![cmf_hook_names::RESOURCE_PRE_FETCH]));
    let observations = plugin.observations();
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;

    runtime
        .handle()
        .before_read_resource("file:///password.env", test_request_context())
        .await
        .expect("resource pre hook runs");

    assert_eq!(1, observations.lock().expect("observations lock poisoned").pre_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn resource_without_post_hook_keeps_its_decision_across_reload() {
    let plugin = Arc::new(TestPlugin::new("resource", vec![cmf_hook_names::RESOURCE_POST_FETCH]));
    let observations = plugin.observations();
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
    runtime.apply_config(None).await.expect("disable hooks");
    let state = runtime
        .handle()
        .before_read_resource("file:///password.env", test_request_context())
        .await
        .expect("request starts");
    runtime.apply_config(Some(plugin_config(&[plugin]).global.expect("global policy"))).await.expect("enable hooks");
    let response = ReadResourceResult::new(vec![ResourceContents::text("original", "file:///password.env")]);
    state.after_read_resource(response).await.expect("in-flight decision survives reload");
    assert_eq!(0, observations.lock().expect("observations lock poisoned").post_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn resource_post_hook_keeps_its_runtime_across_reload() {
    let plugin = Arc::new(TestPlugin::new("resource", vec![cmf_hook_names::RESOURCE_POST_FETCH]).with_post_deny());
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
    let state = runtime
        .handle()
        .before_read_resource("file:///password.env", test_request_context())
        .await
        .expect("request starts");
    runtime.apply_config(None).await.expect("disable hooks");
    let response = ReadResourceResult::new(vec![ResourceContents::text("secret", "file:///password.env")]);
    let error = state.after_read_resource(response).await.expect_err("captured policy still denies");
    assert_eq!("Plugin denied resource", error.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn resource_hooks_preserve_context_across_the_backend_call() {
    let plugin = Arc::new(
        TestPlugin::new("resource", vec![cmf_hook_names::RESOURCE_PRE_FETCH, cmf_hook_names::RESOURCE_POST_FETCH])
            .with_context_roundtrip(),
    );
    let observations = plugin.observations();
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
    let pre = runtime
        .handle()
        .before_read_resource("file:///password.env", test_request_context())
        .await
        .expect("resource pre hook runs");
    let response = ReadResourceResult::new(vec![ResourceContents::text("secret", "file:///password.env")]);

    pre.after_read_resource(response).await.expect("resource post hook receives pre context");

    let observations = observations.lock().expect("observations lock poisoned");
    assert_eq!(1, observations.pre_calls);
    assert_eq!(1, observations.post_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn runtime_config_loads_registered_factory_plugin() {
    let plugin = Arc::new(TestPlugin::new("configured-pre", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite());
    let observations = plugin.observations();
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;

    let result = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook runs");

    assert!(matches!(result.arguments, ArgumentsUpdate::Replace(Some(_))));
    assert_eq!(1, observations.lock().expect("observations lock poisoned").pre_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn runtime_config_loads_generic_cmf_factory_plugin() {
    let config = config_document(json!({
        "plugins": [{
            "name": "generic-pre",
            "kind": "generic",
            "hooks": [cmf_hook_names::TOOL_PRE_INVOKE]
        }]
    }));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config)));
    runtime
        .register_factory("generic", Box::new(CmfPluginFactory::new(TestPlugin::rewrite_from_config)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    let result = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook runs");

    assert!(matches!(result.arguments, ArgumentsUpdate::Replace(Some(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn generic_cmf_factory_registers_prompt_only_plugin() {
    let config = config_document(json!({
        "plugins": [{
            "name": "generic-prompt",
            "kind": "generic",
            "hooks": [cmf_hook_names::PROMPT_PRE_FETCH]
        }]
    }));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config)));
    runtime
        .register_factory("generic", Box::new(CmfPluginFactory::new(TestPlugin::rewrite_from_config)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    let result = runtime
        .handle()
        .before_get_prompt(&review_request("weather"), "review", "backend", test_request_context())
        .await
        .expect("prompt pre hook runs");

    assert!(
        matches!(result.arguments, ArgumentsUpdate::Replace(Some(_))),
        "the prompt hook must actually run, not merely be accepted by config validation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn generic_cmf_factory_registers_mixed_tool_and_prompt_plugin() {
    let config = config_document(json!({
        "plugins": [{
            "name": "generic-mixed",
            "kind": "generic",
            "hooks": [cmf_hook_names::TOOL_PRE_INVOKE, cmf_hook_names::PROMPT_PRE_FETCH]
        }]
    }));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config)));
    runtime
        .register_factory("generic", Box::new(CmfPluginFactory::new(TestPlugin::rewrite_from_config)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    let tool = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("tool pre hook runs");
    let prompt = runtime
        .handle()
        .before_get_prompt(&review_request("weather"), "review", "backend", test_request_context())
        .await
        .expect("prompt pre hook runs");

    assert!(matches!(tool.arguments, ArgumentsUpdate::Replace(Some(_))));
    assert!(matches!(prompt.arguments, ArgumentsUpdate::Replace(Some(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn runtime_reload_replaces_and_clears_current_runtime() {
    let plugin = Arc::new(TestPlugin::new("configured-pre", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite());
    let observations = plugin.observations();
    let config_store = MemoryConfigStore::with_config(config_document(json!({ "plugins": [] })));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));
    runtime
        .register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    let result = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook skips");
    assert!(matches!(result.arguments, ArgumentsUpdate::Unchanged));

    config_store.set_config(plugin_config(&[Arc::clone(&plugin)])).await;
    runtime.reload().await.expect("runtime reloads");
    let result = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook runs");
    assert!(matches!(result.arguments, ArgumentsUpdate::Replace(Some(_))));

    config_store.set_config(config_document(json!({ "plugins": [] }))).await;
    runtime.reload().await.expect("runtime reloads");
    let result = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook skips");
    assert!(matches!(result.arguments, ArgumentsUpdate::Unchanged));
    assert_eq!(1, observations.lock().expect("observations lock poisoned").pre_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn failed_runtime_reload_rejects_new_calls_until_valid_reload() {
    let plugin = Arc::new(TestPlugin::new("configured-pre", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite());
    let observations = plugin.observations();
    let config_store = MemoryConfigStore::with_config(plugin_config(&[Arc::clone(&plugin)]));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));
    runtime
        .register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    config_store
        .set_config(RuntimePluginConfigDocument {
            enabled: true,
            global: None,
            contexts: HashMap::new(),
            settings: RuntimePluginSettings::default(),
        })
        .await;
    runtime.reload().await.expect_err("invalid reload fails");
    let error = expect_runtime_failed(
        runtime.before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context()).await,
    );
    assert_eq!(ErrorCode::INTERNAL_ERROR, error.code);
    assert_eq!("Runtime plugin reload failed", error.message);

    config_store.clear_config().await;
    runtime.reload().await.expect_err("missing reload fails");
    let error = expect_runtime_failed(
        runtime.before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context()).await,
    );
    assert_eq!(ErrorCode::INTERNAL_ERROR, error.code);
    assert_eq!("Runtime plugin reload failed", error.message);

    config_store.set_config(plugin_config(&[Arc::clone(&plugin)])).await;
    runtime.reload().await.expect("runtime recovers");
    let result = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook runs");
    assert!(matches!(result.arguments, ArgumentsUpdate::Replace(Some(_))));
    assert_eq!(1, observations.lock().expect("observations lock poisoned").pre_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn combined_plugin_preserves_context_from_pre_to_post_across_replacement() {
    let plugin = Arc::new(
        TestPlugin::new("context", vec![cmf_hook_names::TOOL_PRE_INVOKE, cmf_hook_names::TOOL_POST_INVOKE])
            .with_context_roundtrip(),
    );
    let observations = plugin.observations();
    let config_store = MemoryConfigStore::with_config(plugin_config(&[Arc::clone(&plugin)]));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));
    runtime
        .register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook runs");
    config_store.set_config(config_document(json!({ "plugins": [] }))).await;
    runtime.reload().await.expect("runtime reloads");
    let response = CallToolResult::success(vec![ContentBlock::text("3")]);
    runtime.after_tool_call("sum", response, pre.state).await.expect("post hook runs");

    let observations = observations.lock().expect("observations lock poisoned");
    assert_eq!(1, observations.post_calls);
    assert_eq!(observations.pre_request_id, observations.post_request_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn post_only_runtime_does_not_apply_new_post_hook_to_in_flight_call() {
    let plugin = Arc::new(TestPlugin::new("post", vec![cmf_hook_names::TOOL_POST_INVOKE]).with_post_rewrite());
    let observations = plugin.observations();
    let config_store = MemoryConfigStore::with_config(config_document(json!({ "plugins": [] })));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));
    runtime
        .register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre hook skips");
    config_store.set_config(plugin_config(&[Arc::clone(&plugin)])).await;
    runtime.reload().await.expect("runtime reloads");
    let response = CallToolResult::success(vec![ContentBlock::text("3")]);
    runtime.after_tool_call("sum", response, pre.state).await.expect("post hook skips");

    assert_eq!(0, observations.lock().expect("observations lock poisoned").post_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn disabled_runtime_does_not_create_tool_post_state() {
    let plugin = Arc::new(TestPlugin::new("post", vec![cmf_hook_names::TOOL_POST_INVOKE]).with_stream_event_rewrite());
    let observations = plugin.observations();
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;

    runtime.apply_config(None).await.expect("disable hooks");
    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("request starts");

    assert!(pre.state.is_none());
    assert_eq!(0, observations.lock().expect("observations lock poisoned").post_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stream_event_is_rewritten_by_post_hook() {
    let plugin = Arc::new(TestPlugin::new("post", vec![cmf_hook_names::TOOL_POST_INVOKE]).with_stream_event_rewrite());
    let observations = plugin.observations();
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;

    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre state is created");
    let event = pre.state.expect("post state").after_stream_event(progress_event()).await.expect("event passes");

    assert_eq!(Some("plugin:step 1/2"), event.expect("event is kept").message.as_deref());
    assert_eq!(1, observations.lock().expect("observations lock poisoned").post_calls);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn denied_stream_event_is_dropped() {
    let plugin = Arc::new(TestPlugin::new("post", vec![cmf_hook_names::TOOL_POST_INVOKE]).with_post_deny());
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;

    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre state is created");
    let event =
        pre.state.expect("post state").after_stream_event(progress_event()).await.expect("deny drops the event");

    assert!(event.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn invalid_stream_event_rewrite_is_rejected() {
    let plugin =
        Arc::new(TestPlugin::new("post", vec![cmf_hook_names::TOOL_POST_INVOKE]).with_invalid_stream_rewrite());
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;

    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("pre state is created");
    let error = pre
        .state
        .expect("post state")
        .after_stream_event(progress_event())
        .await
        .expect_err("invalid rewrite is rejected");

    assert_eq!(ErrorCode::INVALID_PARAMS, error.code);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn replaced_runtime_shutdowns_on_drop() {
    let plugin = Arc::new(TestPlugin::new("pre", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite());
    let observations = plugin.observations();
    let config_store = MemoryConfigStore::with_config(plugin_config(&[Arc::clone(&plugin)]));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));
    runtime
        .register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin)))
        .expect("test factory registers");
    runtime.initialize().await.expect("runtime initializes");

    config_store.set_config(config_document(json!({ "plugins": [] }))).await;
    runtime.reload().await.expect("runtime reloads");

    for _ in 0..TEST_SHUTDOWN_RETRY_COUNT {
        if observations.lock().expect("observations lock poisoned").shutdown_calls > 0 {
            return;
        }
        tokio::time::sleep(TEST_SHUTDOWN_RETRY_INTERVAL).await;
    }
    panic!("replaced runtime did not shut down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn watcher_applies_config_changes() {
    let plugin = Arc::new(TestPlugin::new("configured-pre", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite());
    let observations = plugin.observations();
    let config_store = MemoryConfigStore::with_config(config_document(json!({ "plugins": [] })));
    let mut runtime =
        CpexRuntimeRegistry::with_config_store_interval(Arc::new(config_store.clone()), TEST_WATCHER_INTERVAL);
    runtime
        .register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin)))
        .expect("test factory registers");
    let handle = runtime.initialize().await.expect("runtime initializes");
    assert!(handle.is_some());

    config_store.set_config(plugin_config(&[Arc::clone(&plugin)])).await;
    for _ in 0..TEST_WATCHER_RETRY_COUNT {
        let result = runtime
            .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
            .await
            .expect("pre hook runs");
        if matches!(result.arguments, ArgumentsUpdate::Replace(Some(_))) {
            config_store.clear_config().await;
            tokio::time::sleep(TEST_WATCHER_INTERVAL + TEST_WATCHER_RETRY_INTERVAL).await;
            let error = expect_runtime_failed(
                runtime.before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context()).await,
            );
            assert_eq!(ErrorCode::INTERNAL_ERROR, error.code);

            config_store.set_config(plugin_config(&[Arc::clone(&plugin)])).await;
            for _ in 0..TEST_WATCHER_RETRY_COUNT {
                if let Ok(result) =
                    runtime.before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context()).await
                    && matches!(result.arguments, ArgumentsUpdate::Replace(Some(_)))
                {
                    assert_eq!(2, observations.lock().expect("observations lock poisoned").pre_calls);
                    return;
                }
                tokio::time::sleep(TEST_WATCHER_RETRY_INTERVAL).await;
            }
            panic!("config watcher did not recover from missing plugin config");
        }
        tokio::time::sleep(TEST_WATCHER_RETRY_INTERVAL).await;
    }
    panic!("config watcher did not apply plugin config");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn initialize_without_config_store_returns_no_watcher() {
    let runtime = CpexRuntimeRegistry::default();

    let handle = runtime.initialize().await.expect("runtime initializes");

    assert!(handle.is_none());
}

#[cfg(test)]
impl CpexRuntimeRegistry {
    fn with_config_store(config_store: Arc<dyn RuntimePluginConfigStore>) -> Self {
        Self { config_store: Some(config_store), ..Self::default() }
    }

    fn with_config_store_interval(config_store: Arc<dyn RuntimePluginConfigStore>, watcher_interval: Duration) -> Self {
        Self { config_store: Some(config_store), watcher_interval, ..Self::default() }
    }

    async fn before_tool_call(
        &self,
        request: &CallToolRequestParams,
        tool_name: &str,
        backend_name: &str,
        context: crate::PluginRequestContext,
    ) -> Result<PreHookResult<ToolHookState>, ErrorData> {
        self.handle().before_tool_call(request, tool_name, backend_name, context).await
    }

    async fn after_tool_call(
        &self,
        _tool_name: &str,
        response: CallToolResult,
        state: Option<ToolHookState>,
    ) -> Result<CallToolResult, ErrorData> {
        match state {
            Some(state) => state.after_tool_call(response).await,
            None => Ok(response),
        }
    }
}

fn request_id(payload: &MessagePayload) -> Option<String> {
    payload.message.content.iter().find_map(|part| match part {
        ContentPart::ToolCall { content } => Some(content.tool_call_id.clone()),
        ContentPart::ToolResult { content } => Some(content.tool_call_id.clone()),
        ContentPart::PromptRequest { content } => Some(content.prompt_request_id.clone()),
        ContentPart::PromptResult { content } => Some(content.prompt_request_id.clone()),
        ContentPart::ResourceRef { content } => Some(content.resource_request_id.clone()),
        ContentPart::Resource { content } => Some(content.resource_request_id.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn hook_combinations_preserve_correlation_for_each_operation() {
    use crate::cmf::Operation;
    use rmcp::model::{GetPromptResult, PromptMessage, Role};

    for operation in Operation::ALL {
        for (pre_enabled, post_enabled) in [(false, false), (true, false), (false, true), (true, true)] {
            let [pre, post] = operation.hooks();
            let hooks = [(pre, pre_enabled), (post, post_enabled)]
                .into_iter()
                .filter_map(|(hook, enabled)| enabled.then_some(hook))
                .collect();
            let plugin = Arc::new(TestPlugin::new("combinations", hooks));
            let observations = plugin.observations();
            let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
            let handle = runtime.handle();

            match operation {
                Operation::Tool => {
                    let pre = handle
                        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
                        .await
                        .expect("tool starts");
                    assert_eq!(post_enabled, pre.state.is_some());
                    if let Some(state) = pre.state {
                        state.after_tool_call(CallToolResult::success(vec![])).await.expect("tool finishes");
                    }
                },
                Operation::Prompt => {
                    let pre = handle
                        .before_get_prompt(&review_request("weather"), "review", "backend", test_request_context())
                        .await
                        .expect("prompt starts");
                    assert_eq!(post_enabled, pre.state.is_some());
                    if let Some(state) = pre.state {
                        state
                            .after_get_prompt(GetPromptResult::new(vec![PromptMessage::new_text(
                                Role::User,
                                "weather",
                            )]))
                            .await
                            .expect("prompt finishes");
                    }
                },
                Operation::Resource => {
                    let state = handle
                        .before_read_resource("file:///test", test_request_context())
                        .await
                        .expect("resource starts");
                    state
                        .after_read_resource(ReadResourceResult::new(vec![ResourceContents::text(
                            "weather",
                            "file:///test",
                        )]))
                        .await
                        .expect("resource finishes");
                },
            }
            let observations = observations.lock().expect("observations lock");
            assert_eq!(usize::from(pre_enabled), observations.pre_calls);
            assert_eq!(usize::from(post_enabled), observations.post_calls);
            if pre_enabled && post_enabled {
                assert!(observations.pre_request_id.is_some());
                assert_eq!(observations.pre_request_id, observations.post_request_id);
            }
        }
    }
}

#[tokio::test]
async fn prompt_context_and_policy_survive_a_failed_reload() {
    use rmcp::model::{GetPromptResult, PromptMessage, Role};

    let plugin = Arc::new(
        TestPlugin::new("prompt-context", vec![cmf_hook_names::PROMPT_PRE_FETCH, cmf_hook_names::PROMPT_POST_FETCH])
            .with_context_roundtrip(),
    );
    let config_store = MemoryConfigStore::with_config(plugin_config(&[Arc::clone(&plugin)]));
    let mut runtime = CpexRuntimeRegistry::with_config_store(Arc::new(config_store.clone()));
    runtime.register_factory("test", Box::new(TestPluginFactory::from_plugin(&plugin))).expect("factory registers");
    runtime.initialize().await.expect("runtime initializes");
    let handle = runtime.handle();
    let request = review_request("weather");
    let pre =
        handle.before_get_prompt(&request, "review", "backend", test_request_context()).await.expect("prompt starts");
    config_store.clear_config().await;
    runtime.reload().await.expect_err("missing config fails reload");
    assert!(handle.before_get_prompt(&request, "review", "backend", test_request_context()).await.is_err());
    assert!(handle.before_read_resource("file:///test", test_request_context()).await.is_err());
    pre.state
        .expect("prompt state")
        .after_get_prompt(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, "weather")]))
        .await
        .expect("original prompt context remains usable");
    let observations = plugin.observations.lock().expect("observations lock");
    assert_eq!(1, observations.post_calls);
    assert_eq!(observations.pre_request_id, observations.post_request_id);
}

#[tokio::test]
async fn concurrent_tool_events_share_context_with_the_final_response_after_reload() {
    let mut plugin = TestPlugin::new("event-context", vec![cmf_hook_names::TOOL_POST_INVOKE]);
    plugin.post_behavior = PostBehavior::CountEvents;
    let plugin = Arc::new(plugin);
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("tool starts");
    let state = pre.state.expect("tool state");
    runtime.apply_config(None).await.expect("disable hooks for new calls");
    let (first, second) =
        tokio::join!(state.after_stream_event(progress_event()), state.after_stream_event(progress_event()));
    assert!(first.expect("first event").is_some());
    assert!(second.expect("second event").is_some());
    state.after_tool_call(CallToolResult::success(vec![])).await.expect("both event updates reach the final hook");
    assert_eq!(3, plugin.observations.lock().expect("observations lock").post_calls);
}

#[tokio::test]
async fn dropping_the_last_in_flight_state_releases_the_replaced_runtime() {
    let plugin = Arc::new(TestPlugin::new("pending", vec![cmf_hook_names::TOOL_POST_INVOKE]));
    let runtime = runtime_with_plugin(&plugin, plugin_config(&[Arc::clone(&plugin)])).await;
    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("tool starts");
    let state = pre.state.expect("tool state");
    let event_state = state.clone();
    runtime.apply_config(None).await.expect("replace runtime");
    drop(state);
    tokio::task::yield_now().await;
    assert_eq!(0, plugin.observations.lock().expect("observations lock").shutdown_calls);
    drop(event_state);
    for _ in 0..TEST_SHUTDOWN_RETRY_COUNT {
        if plugin.observations.lock().expect("observations lock").shutdown_calls == 1 {
            return;
        }
        tokio::time::sleep(TEST_SHUTDOWN_RETRY_INTERVAL).await;
    }
    panic!("abandoned request retained its runtime");
}

fn test_request_context() -> crate::PluginRequestContext {
    crate::PluginRequestContext {
        tool: Some(ToolPolicyContext {
            id: "test".to_owned(),
            name: "sum".to_owned(),
            team_id: None,
            context_id: "test".to_owned(),
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn published_hook_policy_controls_payload_writes_without_skipping_execution() {
    use contextforge_data_plane_apis::runtime_plugin_config::HookPayloadPolicy;
    let plugin = Arc::new(TestPlugin::new("policy", vec![cmf_hook_names::TOOL_PRE_INVOKE]).with_pre_rewrite());
    let mut config = plugin_config(&[Arc::clone(&plugin)]);
    config.settings.default_hook_policy = "deny".to_owned();
    let runtime = runtime_with_plugin(&plugin, config.clone()).await;
    let request = sum_request(1, 2);
    let pre =
        runtime.before_tool_call(&request, "sum", "backend", test_request_context()).await.expect("hook executes");
    assert!(matches!(pre.arguments, ArgumentsUpdate::Unchanged));
    assert_eq!(plugin.observations.lock().expect("observations").pre_calls, 1);

    config
        .settings
        .hook_policies
        .insert("tool_pre_invoke".to_owned(), HookPayloadPolicy { writable_fields: ["args".to_owned()].into() });
    runtime.apply_document(config).await.expect("allow argument edits");
    let pre =
        runtime.before_tool_call(&request, "sum", "backend", test_request_context()).await.expect("hook executes");
    assert!(matches!(pre.arguments, ArgumentsUpdate::Replace(Some(_))));
}

#[tokio::test]
async fn explicitly_disabled_plugins_do_not_require_a_factory() {
    let config = config_document(json!({"plugins": [{
        "name": "disabled", "kind": "unavailable", "mode": "disabled", "hooks": ["tool_pre_invoke"],
    }]}));
    let runtime = CpexRuntimeRegistry::with_config_store(Arc::new(MemoryConfigStore::with_config(config)));
    runtime.initialize().await.expect("disabled plugin does not load");
    let pre = runtime
        .before_tool_call(&sum_request(1, 2), "sum", "backend", test_request_context())
        .await
        .expect("call allowed");
    assert!(pre.state.is_none());
    assert!(matches!(pre.arguments, ArgumentsUpdate::Unchanged));
}
