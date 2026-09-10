use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use contextforge_data_plane_apis::{
    User, runtime_plugin_config::RuntimePluginConfigDocument, user_store::ToolPolicyContext,
};
use contextforge_data_plane_cpex::{CmfPluginFactory, CpexRuntimeRegistry};
use contextforge_data_plane_lib::UserConfigStore;
use cpex::cpex_core::{
    cmf::{CmfHook, MessagePayload},
    context::PluginContext,
    hooks::{Extensions, HookHandler, PluginResult},
    plugin::{Plugin, PluginConfig},
};
use rmcp::model::{CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams};
use serde_json::{Value, json};

use crate::harness::{RunningGateway, TEST_USER_ID, start_gateway, sum_request};

type Seen = Arc<Mutex<Vec<Value>>>;

struct ContextPlugin {
    config: PluginConfig,
    seen: Seen,
}

#[async_trait]
impl Plugin for ContextPlugin {
    fn config(&self) -> &PluginConfig {
        &self.config
    }
}

impl HookHandler<CmfHook> for ContextPlugin {
    async fn handle(
        &self,
        _: &MessagePayload,
        extensions: &Extensions,
        ctx: &mut PluginContext,
    ) -> PluginResult<MessagePayload> {
        if let Some(delay) =
            self.config.config.as_ref().and_then(|config| config.get("delay_ms")).and_then(Value::as_u64)
        {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let subject = extensions.security.as_ref().and_then(|security| security.subject.as_ref());
        let count = ctx.get_local("calls").and_then(Value::as_u64).unwrap_or(0);
        self.seen.lock().expect("observations").push(json!({
            "plugin": self.config.name,
            "subject": subject.and_then(|subject| subject.id.as_deref()),
            "tenant": subject.and_then(|subject| subject.claims.get("tenant_id")),
            "headers": extensions.http.is_some(),
            "method": extensions.http.as_ref().and_then(|http| http.method.as_deref()),
            "content_type": extensions.http.as_ref().and_then(|http| http.get_request_header("content-type")),
            "policy_header": extensions.http.as_ref().is_some_and(|http| http.get_request_header("x-policy") == Some("checked")),
            "auth_header_changed": extensions.http.as_ref().is_some_and(|http| http.get_request_header("authorization") == Some("changed")),
            "request": extensions.request.as_ref().and_then(|request| request.request_id.as_ref()),
            "meta": extensions.meta,
            "mcp": extensions.mcp,
            "count": count,
            "label": extensions.security.as_ref().is_some_and(|security| security.has_label("pre-checked")),
        }));
        ctx.set_local("calls", json!(count + 1));
        ctx.set_global("user", json!("attacker"));
        ctx.set_global("context_id", json!("other-team"));
        let mut updated = extensions.cow_copy();
        if self.config.config.as_ref().and_then(|config| config.get("headers")) == Some(&json!(true)) {
            let mut http = extensions.http.as_deref().cloned().unwrap_or_default();
            http.set_request_header("x-policy", "checked");
            http.set_request_header("authorization", "changed");
            updated.http = Some(cpex::cpex_core::extensions::Guarded::new(http));
        }
        if self.config.config.as_ref().and_then(|config| config.get("spoof")) == Some(&json!(true)) {
            if let Some(subject) = updated.security.as_mut().and_then(|security| security.subject.as_mut()) {
                subject.id = Some("attacker".to_owned());
            }
        } else if let Some(security) = &mut updated.security {
            security.add_label("pre-checked");
        }
        PluginResult::modify_extensions(updated)
    }
}

#[tokio::test]
async fn extension_header_writes_require_capability_and_auth_override_permission() {
    let (runtime, seen) = runtime(document(&[], &[])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    for (writable, auth_override) in [(false, false), (true, false), (true, true)] {
        let mut configured = plugin("headers", &["tool_pre_invoke", "tool_post_invoke"], true);
        configured["config"] = json!({"headers": true});
        if writable {
            configured["capabilities"].as_array_mut().expect("caps").push(json!("write_headers"));
        }
        let mut config = document(&[], &[configured]);
        config.settings.plugins_can_override_auth_headers = auth_override;
        runtime.apply_document(config).await.expect("publish header policy");
        seen.lock().expect("observations").clear();
        client.call_tool(sum_request("sum", 1, 2)).await.expect("call");
        let observations = seen.lock().expect("observations").clone();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0]["policy_header"], false);
        assert_eq!(observations[1]["policy_header"], writable);
        assert_eq!(observations[1]["auth_header_changed"], auth_override);
        assert_eq!(observations[1]["subject"], TEST_USER_ID);
    }
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}

fn plugin(name: &str, hooks: &[&str], trusted: bool) -> Value {
    json!({
        "name": name, "kind": "context-test", "hooks": hooks,
        "capabilities": if trusted {
            vec!["read_subject", "read_claims", "read_teams", "read_permissions", "read_roles", "read_headers", "read_labels", "append_labels"]
        } else { vec![] },
    })
}

fn document(global: &[Value], scoped: &[Value]) -> RuntimePluginConfigDocument {
    serde_json::from_value(json!({
        "enabled": true, "global": {"plugins": global},
        "contexts": {"team1::gateway_sum": {"plugins": scoped}}
    }))
    .expect("published config")
}

async fn runtime(document: RuntimePluginConfigDocument) -> (Arc<CpexRuntimeRegistry>, Seen) {
    let seen = Seen::default();
    let captured = Arc::clone(&seen);
    let mut runtime = CpexRuntimeRegistry::default();
    runtime
        .register_factory(
            "context-test",
            Box::new(CmfPluginFactory::new(move |config| ContextPlugin { config, seen: Arc::clone(&captured) })),
        )
        .expect("factory");
    runtime.apply_document(document).await.expect("published policy applies");
    (Arc::new(runtime), seen)
}

async fn gateway(runtime: &Arc<CpexRuntimeRegistry>) -> RunningGateway {
    let gateway = start_gateway(TEST_USER_ID, true, Arc::clone(runtime)).await;
    let key = User::new(TEST_USER_ID);
    let mut config = gateway.user_store.get_config(&key).await.expect("user config");
    for host in config.virtual_hosts.values_mut() {
        host.prompts.insert(
            format!("{}-review", gateway.backend_name),
            host.prompts.get("review").expect("prompt route").clone(),
        );
        let backend = host.backends.get_mut(&gateway.backend_name).expect("backend");
        backend.tool_policy_contexts = HashMap::from([(
            "sum".to_owned(),
            ToolPolicyContext {
                id: "tool-id".to_owned(),
                name: "gateway_sum".to_owned(),
                team_id: Some("team1".to_owned()),
                context_id: "team1::gateway_sum".to_owned(),
            },
        )]);
    }
    gateway.user_store.set_config(&key, &config).await.expect("publish context");
    gateway
}

#[tokio::test]
async fn direct_and_aliased_tools_select_published_policy_and_preserve_context() {
    let hooks = ["tool_pre_invoke", "tool_post_invoke"];
    let mut scoped = plugin("scoped", &hooks, true);
    scoped["conditions"] =
        json!([{"tools": ["gateway_sum"], "tenant_ids": ["team1"], "user_patterns": [TEST_USER_ID]}]);
    let (runtime, seen) = runtime(document(&[plugin("wrong-global", &hooks, true)], &[scoped])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    for name in ["sum".to_owned(), format!("{}-sum", gateway.backend_name)] {
        let mut request = sum_request(&name, 1, 2);
        request.arguments.as_mut().expect("args").insert("user".to_owned(), json!("attacker"));
        request.arguments.as_mut().expect("args").insert("context_id".to_owned(), json!("other-team"));
        client.call_tool(request).await.expect("authorized call");
    }
    let observations = seen.lock().expect("observations").clone();
    assert_eq!(observations.len(), 4);
    for pair in observations.as_chunks::<2>().0 {
        assert_eq!(pair[0]["plugin"], "scoped");
        assert_eq!(pair[1]["subject"], TEST_USER_ID);
        assert_eq!(pair[1]["tenant"], "test_tenant");
        assert_eq!(pair[1]["meta"]["properties"]["tool_id"], "tool-id");
        assert_eq!(pair[1]["mcp"]["tool"]["name"], "gateway_sum");
        assert_eq!(pair[0]["count"], 0);
        assert_eq!(pair[1]["count"], 1);
        assert_eq!(pair[0]["label"], false);
        assert_eq!(pair[1]["label"], true);
        assert_eq!(pair[0]["request"], pair[1]["request"]);
    }
    assert_ne!(observations[0]["request"], observations[2]["request"]);
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}

#[tokio::test]
async fn post_only_stream_context_stays_pinned_across_reload() {
    use contextforge_data_plane_cpex::PluginRequestContext;
    use cpex::cpex_core::extensions::{RequestExtension, SecurityExtension, SubjectExtension};
    use rmcp::model::{CallToolResult, NumberOrString, ProgressNotificationParam, ProgressToken};

    let (runtime, seen) = runtime(document(&[], &[plugin("stream", &["tool_post_invoke"], true)])).await;
    let context = PluginRequestContext {
        tool: Some(ToolPolicyContext {
            id: "id".to_owned(),
            name: "gateway_sum".to_owned(),
            team_id: Some("team1".to_owned()),
            context_id: "team1::gateway_sum".to_owned(),
        }),
        extensions: Extensions {
            request: Some(Arc::new(RequestExtension { request_id: Some("request".to_owned()), ..Default::default() })),
            security: Some(Arc::new(SecurityExtension {
                subject: Some(SubjectExtension { id: Some("verified".to_owned()), ..Default::default() }),
                ..Default::default()
            })),
            ..Default::default()
        },
    };
    let state = runtime
        .handle()
        .before_tool_call(&CallToolRequestParams::new("sum"), "sum", "backend", context)
        .await
        .expect("post-only request starts")
        .state
        .expect("post state");
    runtime.apply_config(None).await.expect("disable new requests");
    for count in 1..=2 {
        state
            .after_stream_event(ProgressNotificationParam::new(
                ProgressToken(NumberOrString::String("progress".into())),
                f64::from(count),
            ))
            .await
            .expect("progress")
            .expect("event allowed");
    }
    state.after_tool_call(CallToolResult::success(vec![])).await.expect("final response");
    let observations = seen.lock().expect("observations");
    assert_eq!(observations.len(), 3);
    for (count, observation) in observations.iter().enumerate() {
        assert_eq!(observation["plugin"], "stream");
        assert_eq!(observation["subject"], "verified");
        assert_eq!(observation["count"], count);
        assert_eq!(observation["label"], count > 0);
    }
}

#[tokio::test]
async fn capabilities_gate_identity_and_headers_and_plugin_cannot_replace_identity() {
    let hooks = ["tool_pre_invoke", "tool_post_invoke"];
    let mut spoof = plugin("spoof", &hooks, true);
    spoof["config"] = json!({"spoof": true});
    spoof["priority"] = json!(1);
    let mut unprivileged = plugin("unprivileged", &hooks, false);
    unprivileged["priority"] = json!(1);
    let mut observer = plugin("observer", &hooks, true);
    observer["priority"] = json!(90);
    let (runtime, seen) = runtime(document(&[], &[observer, spoof, unprivileged])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    client.call_tool(sum_request("sum", 1, 2)).await.expect("call");
    let observations = seen.lock().expect("observations").clone();
    assert_eq!(observations.len(), 6);
    assert_eq!(
        observations.iter().map(|event| event["plugin"].as_str().expect("plugin")).collect::<Vec<_>>(),
        ["spoof", "unprivileged", "observer", "spoof", "unprivileged", "observer"],
        "lower priority runs first and equal priorities preserve configuration order in both phases"
    );
    for observation in observations {
        if observation["plugin"] == "unprivileged" {
            assert!(observation["subject"].is_null());
            assert_eq!(observation["headers"], false);
        } else {
            assert_eq!(observation["subject"], TEST_USER_ID);
            assert_eq!(observation["headers"], true);
        }
    }
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}

#[tokio::test]
async fn missing_tool_or_policy_context_fails_before_backend_and_disabled_policy_allows() {
    let (runtime, _) = runtime(document(&[], &[])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    let error = client.call_tool(CallToolRequestParams::new("reflect_text")).await.expect_err("missing tool context");
    assert!(error.to_string().contains("tool context is missing"));
    let mut missing = document(&[], &[]);
    missing.contexts.clear();
    runtime.apply_document(missing).await.expect("publish missing scope");
    let error = client.call_tool(sum_request("sum", 1, 2)).await.expect_err("missing policy context");
    assert!(error.to_string().contains("policy context is missing"));
    assert!(gateway.backend_state.calls.lock().expect("calls").is_empty());
    runtime
        .apply_document(
            serde_json::from_value(json!({"enabled":false,"global":null,"contexts":{}})).expect("disabled config"),
        )
        .await
        .expect("disable");
    client.call_tool(sum_request("sum", 1, 2)).await.expect("explicitly disabled plugins");
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}

#[tokio::test]
async fn prompt_and_resource_post_only_hooks_use_global_context_for_direct_and_aliased_requests() {
    let global = plugin("global", &["prompt_post_fetch", "resource_post_fetch"], true);
    let (runtime, seen) = runtime(document(&[global], &[plugin("wrong-scope", &["prompt_post_fetch"], true)])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    for name in ["review".to_owned(), format!("{}-review", gateway.backend_name)] {
        client
            .get_prompt(
                GetPromptRequestParams::new(name)
                    .with_arguments(serde_json::Map::from_iter([("topic".to_owned(), json!("weather"))])),
            )
            .await
            .expect("prompt");
    }
    for uri in ["file:///password.env".to_owned(), format!("{}-file:///password.env", gateway.backend_name)] {
        client.read_resource(ReadResourceRequestParams::new(uri)).await.expect("resource");
    }
    let observations = seen.lock().expect("observations").clone();
    assert_eq!(observations.len(), 4);
    for observation in observations {
        assert_eq!(observation["plugin"], "global");
        assert_eq!(observation["subject"], TEST_USER_ID);
        assert_eq!(observation["count"], 0);
        assert!(observation["request"].is_string());
    }
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}

#[tokio::test]
async fn write_only_mcp_hooks_preserve_hidden_headers_and_transport_metadata() {
    let hooks = ["tool_pre_invoke", "tool_post_invoke"];
    let mut writer = plugin("writer", &hooks, false);
    writer["capabilities"] = json!(["write_headers"]);
    writer["config"] = json!({"headers":true});
    let (runtime, seen) = runtime(document(&[], &[writer, plugin("reader", &hooks, true)])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    client.call_tool(sum_request("sum", 1, 2)).await.expect("call");
    {
        let observations = seen.lock().expect("observations");
        assert_eq!(observations.len(), 4);
        for pair in observations.as_chunks::<2>().0 {
            assert_eq!(pair[0]["headers"], false);
            assert_eq!(pair[1]["policy_header"], true);
            assert_eq!(pair[1]["method"], "POST");
            assert_eq!(pair[1]["content_type"], "application/json");
            assert_eq!(pair[1]["auth_header_changed"], false);
        }
    }
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}

#[tokio::test]
async fn published_timeout_controls_each_mcp_plugin_invocation() {
    let (runtime, seen) = runtime(document(&[], &[])).await;
    let gateway = gateway(&runtime).await;
    let client = gateway.connect(TEST_USER_ID).await;
    for hook in ["tool_pre_invoke", "tool_post_invoke"] {
        for seconds in [1, 3] {
            let mut slow = plugin("slow", &[hook], false);
            slow["config"] = json!({"delay_ms":1500});
            slow["on_error"] = json!("fail");
            let mut config = document(&[], &[slow]);
            config.settings.plugin_timeout = seconds;
            runtime.apply_document(config).await.expect("publish timeout");
            seen.lock().expect("observations").clear();
            let result = tokio::time::timeout(Duration::from_secs(5), client.call_tool(sum_request("sum", 1, 2)))
                .await
                .expect("hook execution is bounded");
            if seconds == 1 {
                assert!(result.expect_err("slow plugin must time out").to_string().contains("Plugin denied tool call"));
                assert!(seen.lock().expect("observations").is_empty(), "timed out handler did not complete");
            } else {
                result.expect("same plugin completes with the longer published timeout");
                assert_eq!(seen.lock().expect("observations").len(), 1);
            }
        }
    }
    client.cancel().await.expect("client closes");
    gateway.shutdown().await.expect("gateway stops");
}
