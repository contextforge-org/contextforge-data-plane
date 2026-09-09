use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use contextforge_data_plane_apis::{runtime_plugin_config::RuntimePluginConfigDocument, user_store::ToolPolicyContext};
use contextforge_data_plane_cpex::{
    CpexRuntimeRegistry, GatewayPluginFactory, HttpHook, HttpHookPayload, PluginRequestContext,
};
use cpex::cpex_core::{
    cmf::{CmfHook, MessagePayload},
    context::PluginContext,
    error::PluginViolation,
    extensions::{Extensions, HttpExtension, RequestExtension, SecurityExtension, SubjectExtension},
    hooks::{HookHandler, PluginResult},
    plugin::{Plugin, PluginConfig},
};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Value, json};

use crate::harness::{MemoryUserConfigStore, TEST_USER_ID, TestServer, create_default_config, token};
use contextforge_data_plane_lib::{
    AuthorizationClaims, AuthorizationService, Gateway, UserConfigStore, UserConfigStoreType,
};

#[derive(Debug)]
struct VerifyJwt;
#[async_trait]
impl AuthorizationService for VerifyJwt {
    async fn authorize(&self, header: &http::HeaderValue) -> Option<AuthorizationClaims> {
        let token = header.to_str().ok()?.strip_prefix("Bearer ")?;
        let key = jsonwebtoken::DecodingKey::from_rsa_pem(&std::fs::read("../../assets/jwt.key.pub").ok()?).ok()?;
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_audience(&["mcpgateway-api"]);
        validation.set_issuer(&["mcpgateway"]);
        jsonwebtoken::decode::<Value>(token, &key, &validation).ok().map(|data| AuthorizationClaims::from(data.claims))
    }
}

async fn start_verified_gateway(runtime: Arc<CpexRuntimeRegistry>) -> TestServer {
    let store = MemoryUserConfigStore::default();
    store
        .set_config(
            &contextforge_data_plane_apis::User::new(TEST_USER_ID),
            &serde_json::from_value(json!({
                "virtual_hosts":{"test":{"backends":{}}}
            }))
            .expect("user config"),
        )
        .await
        .expect("store config");
    let router = Gateway::builder()
        .with_config(create_default_config())
        .with_session_manager(Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ))
        .with_user_config_store_type(UserConfigStoreType::Test(Arc::new(store)))
        .with_plugin_runtime(Some(runtime.handle()))
        .with_authorization_service(Arc::new(VerifyJwt))
        .build()
        .into_router()
        .await
        .expect("router");
    TestServer::start_http(router).await.expect("server")
}

type Seen = Arc<Mutex<Vec<Value>>>;
struct Probe {
    config: PluginConfig,
    seen: Seen,
}

#[async_trait]
impl Plugin for Probe {
    fn config(&self) -> &PluginConfig {
        &self.config
    }
}

impl Probe {
    fn observe(&self, stage: &str, extensions: &Extensions, ctx: &mut PluginContext) {
        let count = ctx.get_local("count").and_then(Value::as_u64).unwrap_or(0);
        self.seen.lock().expect("observations").push(json!({
            "plugin": self.config.name,
            "stage": stage, "count": count, "global": ctx.get_global("count"),
            "subject": extensions.security.as_ref().and_then(|security| security.subject.as_ref()).and_then(|subject| subject.id.as_deref()),
            "request": extensions.request.as_ref().and_then(|request| request.request_id.as_deref()),
            "headers_visible": extensions.http.is_some(),
            "request_content_type": extensions.http.as_ref().and_then(|http| http.get_request_header("content-type")),
            "response_content_type": extensions.http.as_ref().and_then(|http| http.get_response_header("content-type")),
        }));
        ctx.set_local("count", json!(count + 1));
        ctx.set_global("count", json!(count + 1));
    }
}

#[allow(clippy::unused_async_trait_impl)]
impl HookHandler<CmfHook> for Probe {
    async fn handle(
        &self,
        _: &MessagePayload,
        extensions: &Extensions,
        ctx: &mut PluginContext,
    ) -> PluginResult<MessagePayload> {
        self.observe("mcp", extensions, ctx);
        PluginResult::allow()
    }
}

impl HookHandler<HttpHook> for Probe {
    async fn handle(
        &self,
        payload: &HttpHookPayload,
        extensions: &Extensions,
        ctx: &mut PluginContext,
    ) -> PluginResult<HttpHookPayload> {
        let post = payload.status_code.is_some();
        self.observe(if post { "http_post" } else { "http_pre" }, extensions, ctx);
        let config = self.config.config.clone().unwrap_or_default();
        if config.get("timeout") == Some(&json!(true)) {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        if config.get("deny") == Some(&json!(true)) {
            return PluginResult::deny(PluginViolation::new("test", "HTTP hook failure"));
        }
        let mut updated = extensions.cow_copy();
        // Returning hidden fields is intentional: the host must enforce capabilities.
        let mut http = extensions.http.as_deref().cloned().unwrap_or_default();
        if post {
            http.set_response_header("x-plugin-status", payload.status_code.expect("post status").to_string());
            http.set_response_header("x-plugin-count", ctx.get_local("count").expect("count").to_string());
        } else {
            http.set_request_header("x-from-plugin", "present");
            if let Some(auth) = config.get("authorization").and_then(Value::as_str) {
                http.set_request_header("Authorization", auth);
            }
            if config.get("invalid") == Some(&json!(true)) {
                http.set_request_header("x-invalid", "bad\r\nheader");
            }
            if config.get("oversized") == Some(&json!(true)) {
                http.set_request_header("Mcp-Name", "x".repeat(100_000));
            }
        }
        updated.http = Some(cpex::cpex_core::extensions::Guarded::new(http));
        PluginResult::modify_extensions(updated)
    }
}

fn configured(hooks: &[&str], config: Value) -> Value {
    let mut plugin = json!({"name":"probe", "kind":"http-probe", "hooks":hooks,
        "capabilities":["read_headers","write_headers","read_subject"]});
    plugin["config"] = config;
    plugin
}

fn document(global: Value, scoped: Value) -> RuntimePluginConfigDocument {
    let mut document = json!({"enabled":true, "global":{}, "contexts":{"team::sum":{}}});
    document["global"]["plugins"] = global;
    document["contexts"]["team::sum"]["plugins"] = scoped;
    serde_json::from_value(document).expect("document")
}

async fn runtime(document: RuntimePluginConfigDocument) -> (Arc<CpexRuntimeRegistry>, Seen) {
    let seen = Seen::default();
    let captured = Arc::clone(&seen);
    let mut registry = CpexRuntimeRegistry::default();
    registry
        .register_factory(
            "http-probe",
            Box::new(
                GatewayPluginFactory::new(move |config| Probe { config, seen: Arc::clone(&captured) })
                    .with_cmf_hooks()
                    .with_http_hooks(),
            ),
        )
        .expect("factory");
    registry.apply_document(document).await.expect("configure");
    (Arc::new(registry), seen)
}

fn http_extensions() -> Extensions {
    Extensions {
        request: Some(Arc::new(RequestExtension {
            request_id: Some("shared-request".to_owned()),
            ..Default::default()
        })),
        http: Some(Arc::new(HttpExtension {
            method: Some("POST".to_owned()),
            path: Some("/mcp".to_owned()),
            ..Default::default()
        })),
        security: Some(Arc::new(SecurityExtension::default())),
        ..Default::default()
    }
}

#[tokio::test]
async fn http_and_scoped_mcp_hooks_preserve_priority_and_state_across_reload() {
    let plugins = |hooks: &[&str]| {
        [("later", 90), ("earlier", 1), ("equal-priority", 1)].map(|(name, priority)| {
            let mut plugin = configured(hooks, json!({}));
            plugin["name"] = json!(name);
            plugin["priority"] = json!(priority);
            plugin
        })
    };
    let global = plugins(&["http_pre_request", "http_post_request"]);
    let scoped = plugins(&["tool_pre_invoke", "tool_post_invoke"]);
    let (registry, seen) = runtime(document(json!(global), json!(scoped))).await;
    let http = registry
        .handle()
        .before_http_request(
            HttpHookPayload {
                method: "POST".to_owned(),
                path: "/mcp".to_owned(),
                client_addr: None,
                status_code: None,
            },
            http_extensions(),
        )
        .await
        .expect("HTTP before");
    registry.apply_config(None).await.expect("reload between HTTP and MCP");
    http.request.set_verified_subject(SubjectExtension { id: Some("verified".to_owned()), ..Default::default() }).await;
    let state = registry
        .handle()
        .before_tool_call(
            &CallToolRequestParams::new("sum"),
            "sum",
            "backend",
            PluginRequestContext {
                request: Some(http.request.clone()),
                tool: Some(ToolPolicyContext {
                    id: "tool-id".to_owned(),
                    name: "sum".to_owned(),
                    team_id: Some("team".to_owned()),
                    context_id: "team::sum".to_owned(),
                }),
                extensions: Extensions {
                    security: Some(Arc::new(SecurityExtension {
                        subject: Some(SubjectExtension {
                            id: Some("stale-route-copy".to_owned()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            },
        )
        .await
        .expect("MCP before uses pinned scoped config")
        .state
        .expect("post state");
    state.after_tool_call(CallToolResult::success(vec![])).await.expect("MCP after");
    let extensions = http.after_http_request(200, HashMap::new()).await;
    assert_eq!(extensions.http.as_ref().expect("headers").get_response_header("x-plugin-count"), Some("4"));
    let observations = seen.lock().expect("observations");
    assert_eq!(observations.len(), 12);
    for (index, observation) in observations.iter().enumerate() {
        let count = index / 3;
        assert_eq!(observation["plugin"], ["earlier", "equal-priority", "later"][index % 3]);
        assert_eq!(observation["count"], count);
        assert_eq!(observation["request"], "shared-request");
        assert_eq!(observation["subject"], if count == 0 { Value::Null } else { json!("verified") });
    }
}

#[tokio::test]
async fn write_only_http_hooks_preserve_headers_and_transport_metadata() {
    for auth_override in [false, true] {
        let mut writer = configured(&["http_pre_request", "http_post_request"], json!({"authorization":"changed"}));
        writer["name"] = json!("writer");
        writer["capabilities"] = json!(["write_headers"]);
        writer["priority"] = json!(1);
        let reader = configured(&["http_pre_request", "http_post_request"], json!({}));
        let mut config = document(json!([writer, reader]), json!([]));
        config.settings.plugins_can_override_auth_headers = auth_override;
        let (registry, seen) = runtime(config).await;
        let mut extensions = http_extensions();
        let http = Arc::make_mut(extensions.http.as_mut().expect("HTTP metadata"));
        http.host = Some("gateway.example".to_owned());
        http.scheme = Some("https".to_owned());
        http.set_request_header("content-type", "application/json");
        http.set_request_header("authorization", "original");
        http.set_request_header("x-from-plugin", "old");
        let state = registry
            .handle()
            .before_http_request(
                HttpHookPayload {
                    method: "POST".to_owned(),
                    path: "/mcp".to_owned(),
                    client_addr: None,
                    status_code: None,
                },
                extensions,
            )
            .await
            .expect("HTTP before");
        let extensions = state
            .after_http_request(
                200,
                HashMap::from([
                    ("content-type".to_owned(), "application/json".to_owned()),
                    ("x-plugin-status".to_owned(), "old".to_owned()),
                ]),
            )
            .await;
        let http = extensions.http.expect("HTTP metadata survives blind writes");
        assert_eq!(http.method.as_deref(), Some("POST"));
        assert_eq!(http.path.as_deref(), Some("/mcp"));
        assert_eq!(http.host.as_deref(), Some("gateway.example"));
        assert_eq!(http.scheme.as_deref(), Some("https"));
        assert_eq!(http.get_request_header("content-type"), Some("application/json"));
        assert_eq!(http.get_response_header("content-type"), Some("application/json"));
        assert_eq!(http.get_request_header("x-from-plugin"), Some("present"));
        assert_eq!(http.get_response_header("x-plugin-status"), Some("200"));
        assert_eq!(http.get_request_header("authorization"), Some(if auth_override { "changed" } else { "original" }));
        let observations = seen.lock().expect("observations");
        assert_eq!(observations.len(), 4);
        for index in [0, 2] {
            assert_eq!(observations[index]["headers_visible"], false, "write access must not grant read access");
            assert_eq!(observations[index + 1]["request_content_type"], "application/json");
        }
        assert_eq!(observations[3]["response_content_type"], "application/json");
    }
}

async fn send(gateway: &TestServer, authorization: Option<&str>) -> reqwest::Response {
    let client = reqwest::Client::new();
    let request = client
        .post(gateway.url("/contextforge-rs/servers/test/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}));
    let request = if let Some(auth) = authorization { request.header("Authorization", auth) } else { request };
    request.send().await.expect("HTTP response")
}

#[tokio::test]
async fn request_header_edits_run_before_auth_and_response_hooks_cover_auth_failures() {
    let valid = format!("Bearer {}", token(TEST_USER_ID));
    let probe = configured(&["http_pre_request", "http_post_request"], json!({"authorization":valid}));
    let (registry, seen) = runtime(document(json!([probe]), json!([]))).await;
    let gateway = start_verified_gateway(Arc::clone(&registry)).await;
    let response = send(&gateway, None).await;
    assert_eq!(response.status(), 200, "plugin supplies a credential which normal authentication verifies");
    assert_eq!(response.headers()["x-plugin-status"], "200");
    {
        let events = seen.lock().expect("observations");
        assert!(events[0]["subject"].is_null());
        assert_eq!(events[1]["subject"], TEST_USER_ID, "HTTP after sees verified identity without any MCP plugin");
    }
    let response = send(&gateway, Some("Bearer invalid")).await;
    assert_eq!(response.status(), 401, "plugin cannot replace an existing credential by default");
    assert_eq!(response.headers()["x-plugin-status"], "401");
    let missing_user = format!("Bearer {}", token("missing-config"));
    assert_eq!(send(&gateway, Some(&missing_user)).await.status(), 400);
    assert_eq!(
        seen.lock().expect("observations").last().expect("HTTP after")["subject"],
        "missing-config",
        "verified identity survives user configuration lookup failure"
    );
    let mut override_config = document(
        json!([configured(&["http_pre_request", "http_post_request"], json!({"authorization":valid}))]),
        json!([]),
    );
    override_config.settings.plugins_can_override_auth_headers = true;
    registry.apply_document(override_config).await.expect("allow credential replacement");
    assert_eq!(send(&gateway, Some("Bearer invalid")).await.status(), 200);
    gateway.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn capabilities_and_published_header_policy_are_enforced() {
    for (capabilities, policy_allows) in [(false, true), (true, false)] {
        let mut probe = configured(&["http_pre_request", "http_post_request"], json!({}));
        if !capabilities {
            probe["capabilities"] = json!([]);
        }
        let mut config = document(json!([probe]), json!([]));
        if !policy_allows {
            config.settings.default_hook_policy = "deny".to_owned();
        }
        let (registry, seen) = runtime(config).await;
        let gateway = start_verified_gateway(registry).await;
        let response = send(&gateway, None).await;
        assert_eq!(response.status(), 401);
        assert!(response.headers().get("x-plugin-status").is_none());
        assert!(
            seen.lock().expect("observations").iter().all(|observation| observation["headers_visible"] == capabilities)
        );
        gateway.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn post_only_hooks_run_and_invalid_header_edits_are_ignored() {
    for (hooks, config) in [
        (vec!["http_post_request"], json!({})),
        (
            vec!["http_pre_request", "http_post_request"],
            json!({"invalid":true,"authorization":format!("Bearer {}", token(TEST_USER_ID))}),
        ),
    ] {
        let (registry, _) = runtime(document(json!([configured(&hooks, config)]), json!([]))).await;
        let gateway = start_verified_gateway(registry).await;
        let response = send(&gateway, None).await;
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()["x-plugin-status"], "401");
        gateway.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn denied_or_timed_out_http_hooks_continue_but_header_limits_still_apply() {
    for config in [json!({"deny":true}), json!({"timeout":true}), json!({"oversized":true})] {
        let oversized = config.get("oversized").is_some();
        let mut config = document(json!([configured(&["http_pre_request", "http_post_request"], config)]), json!([]));
        config.settings.plugin_timeout = 1;
        let (registry, _) = runtime(config).await;
        let gateway = start_verified_gateway(registry).await;
        let response =
            tokio::time::timeout(Duration::from_secs(5), send(&gateway, None)).await.expect("bounded hook execution");
        assert_eq!(response.status().as_u16(), if oversized { 431 } else { 401 });
        gateway.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn streaming_http_after_runs_before_a_long_running_tool_finishes() {
    let probe = configured(
        &["http_pre_request", "http_post_request", "cmf.tool_pre_invoke", "cmf.tool_post_invoke"],
        json!({}),
    );
    let (registry, seen) = runtime(document(json!([probe.clone()]), json!([]))).await;
    // This fixture intentionally uses one static policy; the scoped case is covered above.
    registry
        .apply_config(Some(serde_json::from_value(json!({"plugins":[probe]})).expect("config")))
        .await
        .expect("static policy");
    let gateway = crate::harness::start_gateway(TEST_USER_ID, true, registry).await;
    let request = reqwest::Client::new()
        .post(gateway.gateway_url())
        .bearer_auth(token(TEST_USER_ID))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "wait_for_cancellation")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"wait_for_cancellation","arguments":{},"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}, "progressToken":"http-streaming"
            }
        }}));
    let response = tokio::time::timeout(Duration::from_secs(2), request.send())
        .await
        .expect("headers must not wait for tool completion")
        .expect("HTTP response");
    assert_eq!(response.status(), 200);
    assert!(response.headers()["content-type"].to_str().expect("content type").starts_with("text/event-stream"));
    assert_eq!(response.headers()["x-plugin-status"], "200");
    {
        let events = seen.lock().expect("observations");
        // The MCP post hook also processes the first progress notification.
        assert_eq!(
            events.iter().map(|event| event["stage"].as_str().expect("stage")).collect::<Vec<_>>(),
            ["http_pre", "mcp", "mcp", "http_post"]
        );
        for (count, event) in events.iter().enumerate() {
            assert_eq!(event["count"], count, "real HTTP and MCP hooks share local state");
            assert_eq!(event["request"], events[0]["request"]);
        }
    }
    drop(response);
    gateway.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn prompt_and_resource_hooks_use_the_same_request_lifecycle_with_global_policy() {
    use rmcp::model::{GetPromptRequestParams, GetPromptResult, ReadResourceResult};
    let probe =
        configured(&["http_pre_request", "http_post_request", "prompt_post_fetch", "resource_post_fetch"], json!({}));
    let (registry, seen) = runtime(document(json!([probe]), json!([]))).await;
    for prompt in [true, false] {
        let http = registry
            .handle()
            .before_http_request(
                HttpHookPayload {
                    method: "POST".to_owned(),
                    path: "/mcp".to_owned(),
                    client_addr: None,
                    status_code: None,
                },
                http_extensions(),
            )
            .await
            .expect("HTTP before");
        let context = PluginRequestContext { request: Some(http.request.clone()), ..Default::default() };
        if prompt {
            registry
                .handle()
                .before_get_prompt(&GetPromptRequestParams::new("review"), "review", "backend", context)
                .await
                .expect("prompt before")
                .state
                .expect("post-only state")
                .after_get_prompt(GetPromptResult::new(vec![]))
                .await
                .expect("prompt after");
        } else {
            registry
                .handle()
                .before_read_resource("file:///test", context)
                .await
                .expect("resource before")
                .after_read_resource(ReadResourceResult::new(vec![]))
                .await
                .expect("resource after");
        }
        http.after_http_request(200, HashMap::new()).await;
    }
    let observations = seen.lock().expect("observations");
    assert_eq!(observations.len(), 6);
    for (index, event) in observations.iter().enumerate() {
        assert_eq!(event["count"], index % 3, "state is shared within each request and isolated between requests");
    }
}
