use crate::harness::{TestServer, create_default_config};
use async_trait::async_trait;
use axum::{Json, Router, body::Body, routing::get};
use contextforge_data_plane_apis::{
    User,
    user_store::{UserConfig, VirtualHost},
};
use contextforge_data_plane_lib::{
    Config, ConfigStore, ConfigStoreError, Gateway, UserConfigStoreType, get_authorization_service,
};
use http::{Request, StatusCode};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, jwk::Jwk};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

fn key() -> EncodingKey {
    EncodingKey::from_rsa_pem(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/jwt.key")))
        .expect("valid authentication test fixture")
}
fn claims() -> Value {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("valid authentication test fixture").as_secs();
    json!({"iss":"mcpgateway", "aud":"mcpgateway-api", "sub":"user", "woTenantId":"tenant", "tenant_id":"tenant", "role":"user", "exp":now+3600, "nbf":now-60})
}
fn token(claims: &Value, kid: Option<&str>, algorithm: Algorithm) -> String {
    let mut header = Header::new(algorithm);
    header.kid = kid.map(str::to_owned);
    encode(&header, claims, &key()).expect("valid authentication test fixture")
}

#[derive(Clone, Default)]
struct CountingStore {
    reads: Arc<AtomicUsize>,
    virtual_host: Option<VirtualHost>,
}
#[async_trait]
impl ConfigStore<User, UserConfig> for CountingStore {
    async fn get_config<'a>(&self, user: &'a User) -> Result<UserConfig, ConfigStoreError> {
        assert_eq!(user.key(), "user");
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(UserConfig {
            virtual_hosts: HashMap::from([(
                "test".into(),
                self.virtual_host.clone().unwrap_or_else(|| VirtualHost {
                    backends: HashMap::new(),
                    tools: HashMap::new(),
                    resources: HashMap::new(),
                    resource_templates: HashMap::new(),
                    prompts: HashMap::new(),
                }),
            )]),
        })
    }
    async fn set_config<'a>(&self, _: &'a User, _: &'a UserConfig) -> Result<(), ConfigStoreError> {
        unreachable!()
    }
}

async fn gateway(config: Config, store: CountingStore) -> Router {
    Gateway::builder()
        .with_authorization_service(
            get_authorization_service(&config.jwks_config).expect("valid authentication test fixture"),
        )
        .with_config(config)
        .with_user_config_store_type(UserConfigStoreType::Test(Arc::new(store)))
        .with_session_manager(Arc::new(LocalSessionManager::default()))
        .build()
        .into_router()
        .await
        .expect("valid authentication test fixture")
}

fn request(token: Option<&str>, method: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/contextforge-rs/servers/test/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", method);
    if method == "tools/call" {
        request = request.header("Mcp-Name", "sum");
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut params = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "auth-test", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    });
    if method == "tools/call" {
        params["name"] = "sum".into();
        params["arguments"] = json!({"a": 2, "b": 3});
    }
    request
        .body(Body::from(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()))
        .expect("valid authentication test fixture")
}

async fn key_server() -> TestServer {
    let mut jwk = Jwk::from_encoding_key(&key(), Algorithm::RS256).expect("valid authentication test fixture");
    jwk.common.key_id = Some("test".into());
    let document = json!({"keys":[jwk]});
    TestServer::start_http(Router::new().route(
        "/jwks",
        get(move || {
            let document = document.clone();
            async move { Json(document) }
        }),
    ))
    .await
    .expect("valid authentication test fixture")
}

#[tokio::test]
async fn permission_denial_prevents_backend_calls_after_a_successful_request() {
    use axum::{
        extract::State,
        middleware::{self, Next},
    };
    use contextforge_data_plane_apis::user_store::{BackendMCPGateway, ServiceRoute};
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
    let hits = Arc::new(AtomicUsize::new(0));
    let service = StreamableHttpService::new(
        || Ok(crate::harness::mock_counter::Counter::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let backend =
        TestServer::start_http(Router::new().route_service("/mcp", service).layer(middleware::from_fn_with_state(
            Arc::clone(&hits),
            |State(hits): State<Arc<AtomicUsize>>, request: axum::extract::Request, next: Next| async move {
                if request.headers().get("Mcp-Method").is_some_and(|method| method == "tools/call") {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                next.run(request).await
            },
        )))
        .await
        .expect("valid authentication test fixture");
    let store = CountingStore {
        virtual_host: Some(VirtualHost {
            backends: HashMap::from([(
                "counter".into(),
                BackendMCPGateway {
                    name: "counter".into(),
                    url: backend.url("/mcp").parse().expect("valid authentication test fixture"),
                    mcp_protocol_version: rmcp::model::ProtocolVersion::V_2026_07_28,
                    passthrough_headers: vec![],
                    add_headers: HashMap::new(),
                    remove_headers: vec![],
                    tool_schemas: HashMap::new(),
                    completion: HashMap::new(),
                },
            )]),
            tools: HashMap::from([(
                "sum".into(),
                ServiceRoute { backend_name: "counter".into(), upstream_name: "sum".into() },
            )]),
            resources: HashMap::new(),
            resource_templates: HashMap::new(),
            prompts: HashMap::new(),
        }),
        ..Default::default()
    };
    let server = key_server().await;
    let mut config = create_default_config();
    config.upstream_transport_config.upstream_connection_mode =
        Some(contextforge_data_plane_lib::UpstreamConnectionMode::PlainTextOrTls);
    config.jwks_config.url = server.url("/jwks").parse().expect("valid authentication test fixture");
    let app = gateway(config, store.clone()).await;
    for (role, expected) in
        [("user", StatusCode::OK), ("unknown", StatusCode::FORBIDDEN), ("expired", StatusCode::UNAUTHORIZED)]
    {
        let mut c = claims();
        c["role"] = role.into();
        if role == "expired" {
            c["exp"] = json!(1);
        }
        let token = token(&c, Some("test"), Algorithm::RS256);
        let request = request(Some(&token), "tools/call");
        let response = app.clone().oneshot(request).await.expect("valid authentication test fixture");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 65_536).await.expect("valid authentication test fixture");
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
        if role == "user" {
            let text = String::from_utf8_lossy(&body);
            let data = text.lines().find_map(|line| line.strip_prefix("data: ")).unwrap_or(&text);
            let message: Value = serde_json::from_str(data).expect("MCP result JSON");
            assert_eq!(message["result"]["content"][0]["text"], "5", "{message}");
        }
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1, "denied request must never reach backend");
    }
    let response = app.oneshot(request(None, "tools/call")).await.expect("authentication fixture");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.shutdown().await.expect("valid authentication test fixture");
    backend.shutdown().await.expect("valid authentication test fixture");
}
