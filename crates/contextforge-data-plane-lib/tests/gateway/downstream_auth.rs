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

async fn key_server() -> (TestServer, Arc<AtomicUsize>) {
    let mut jwk = Jwk::from_encoding_key(&key(), Algorithm::RS256).expect("valid authentication test fixture");
    jwk.common.key_id = Some("test".into());
    let document = json!({"keys":[jwk]});
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    let server = TestServer::start_http(Router::new().route(
        "/jwks",
        get(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let document = document.clone();
            async move { Json(document) }
        }),
    ))
    .await
    .expect("valid authentication test fixture");
    (server, requests)
}

#[tokio::test]
async fn authenticates_real_rsa_tokens_and_rejects_before_configuration() {
    let (server, requests) = key_server().await;
    let mut config = create_default_config();
    config.jwks_config.url = server.url("/jwks").parse().expect("valid authentication test fixture");
    let store = CountingStore::default();
    let app = gateway(config, store.clone()).await;
    // Warm both authentication and the request path, then deny access on every subsequent request.
    let valid = token(&claims(), Some("test"), Algorithm::RS256);
    let response =
        app.clone().oneshot(request(Some(&valid), "server/discover")).await.expect("valid authentication test fixture");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 65_536).await.expect("valid authentication test fixture");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    for method in ["server/discover", "tools/list", "tools/call", "resources/list", "prompts/list"] {
        let mut denied = claims();
        denied["role"] = "unknown".into();
        let denied = token(&denied, Some("test"), Algorithm::RS256);
        assert_eq!(
            app.clone()
                .oneshot(request(Some(&denied), method))
                .await
                .expect("valid authentication test fixture")
                .status(),
            StatusCode::FORBIDDEN,
            "{method}"
        );
    }
    let mut invalid = vec![
        None,
        Some("malformed".into()),
        Some(token(&claims(), None, Algorithm::RS256)),
        Some(token(&claims(), Some("unknown"), Algorithm::RS256)),
        Some(token(&claims(), Some("test"), Algorithm::RS384)),
    ];
    for (name, value) in [
        ("iss", json!("wrong")),
        ("aud", json!("wrong")),
        ("exp", json!(1)),
        ("nbf", json!(9_999_999_999_u64)),
        ("nbf", json!("tomorrow")),
        ("sub", json!("")),
        ("woTenantId", json!(null)),
        ("tenant_id", json!("conflicting")),
    ] {
        let mut c = claims();
        c[name] = value;
        invalid.push(Some(token(&c, Some("test"), Algorithm::RS256)));
    }
    for missing in ["iss", "aud", "exp", "sub", "woTenantId"] {
        let mut c = claims();
        c.as_object_mut().expect("valid authentication test fixture").remove(missing);
        if missing == "woTenantId" {
            c.as_object_mut().expect("claims object").remove("tenant_id");
        }
        invalid.push(Some(token(&c, Some("test"), Algorithm::RS256)));
    }
    let mut tampered = valid.clone().into_bytes();
    let position = tampered.iter().rposition(|b| *b == b'.').expect("valid authentication test fixture") + 1;
    tampered[position] = if tampered[position] == b'A' { b'B' } else { b'A' };
    invalid.push(Some(String::from_utf8(tampered).expect("valid authentication test fixture")));
    for token in invalid {
        let response = app
            .clone()
            .oneshot(request(token.as_deref(), "server/discover"))
            .await
            .expect("valid authentication test fixture");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
    }
    let mut ambiguous = request(Some(&valid), "server/discover");
    ambiguous
        .headers_mut()
        .append("authorization", format!("Bearer {valid}").parse().expect("valid authentication test fixture"));
    assert_eq!(
        app.clone().oneshot(ambiguous).await.expect("valid authentication test fixture").status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(store.reads.load(Ordering::SeqCst), 1, "denials must not access configuration");
    assert_eq!(requests.load(Ordering::SeqCst), 1, "valid, invalid and unknown kid traffic uses a bounded cache");
    for role in ["admin", "builder", "user"] {
        let mut c = claims();
        c["role"] = role.into();
        c["aud"] = json!(["unrelated", "mcpgateway-api"]);
        let token = token(&c, Some("test"), Algorithm::RS256);
        assert_eq!(
            app.clone()
                .oneshot(request(Some(&token), "server/discover"))
                .await
                .expect("valid authentication test fixture")
                .status(),
            StatusCode::OK
        );
    }
    server.shutdown().await.expect("valid authentication test fixture");
}

#[tokio::test]
async fn jwks_outage_is_503_and_never_reaches_configuration() {
    let requests = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&requests);
    let server = TestServer::start_http(Router::new().route(
        "/jwks",
        get(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::SERVICE_UNAVAILABLE }
        }),
    ))
    .await
    .expect("valid authentication test fixture");
    let mut config = create_default_config();
    config.jwks_config.url = server.url("/jwks").parse().expect("valid authentication test fixture");
    let store = CountingStore::default();
    let app = gateway(config, store.clone()).await;
    let token = token(&claims(), Some("test"), Algorithm::RS256);
    let results =
        futures::future::join_all((0..20).map(|_| app.clone().oneshot(request(Some(&token), "server/discover")))).await;
    for result in results {
        assert_eq!(result.expect("valid authentication test fixture").status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    server.shutdown().await.expect("valid authentication test fixture");
}

/// Small repeatable load probe of the complete cached-auth/discovery path.
/// Run explicitly with --ignored --nocapture; compare the same profile on both revisions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual cached-authentication load comparison"]
async fn cached_authentication_load() {
    let (server, requests) = key_server().await;
    let mut config = create_default_config();
    config.jwks_config.url = server.url("/jwks").parse().expect("valid authentication test fixture");
    let store = CountingStore::default();
    let app = gateway(config, store).await;
    let token = token(&claims(), Some("test"), Algorithm::RS256);
    for _ in 0..100 {
        assert_eq!(
            app.clone()
                .oneshot(request(Some(&token), "server/discover"))
                .await
                .expect("valid authentication test fixture")
                .status(),
            StatusCode::OK
        );
    }
    let start = std::time::Instant::now();
    let tasks = (0..16)
        .map(|_| {
            let app = app.clone();
            let token = token.clone();
            tokio::spawn(async move {
                let mut times = Vec::with_capacity(500);
                for _ in 0..500 {
                    let start = std::time::Instant::now();
                    let response = app
                        .clone()
                        .oneshot(request(Some(&token), "server/discover"))
                        .await
                        .expect("valid authentication test fixture");
                    assert_eq!(response.status(), StatusCode::OK);
                    let _ = axum::body::to_bytes(response.into_body(), 65_536)
                        .await
                        .expect("valid authentication test fixture");
                    times.push(start.elapsed().as_micros());
                }
                times
            })
        })
        .collect::<Vec<_>>();
    let mut times = Vec::new();
    for task in tasks {
        times.extend(task.await.expect("valid authentication test fixture"));
    }
    let elapsed = start.elapsed();
    times.sort_unstable();
    println!(
        "cached auth/discovery: requests=8000 concurrency=16 elapsed_ms={} rps={:.0} p50_us={} p95_us={} jwks_fetches={}",
        elapsed.as_millis(),
        8000.0 / elapsed.as_secs_f64(),
        times[4000],
        times[7600],
        requests.load(Ordering::SeqCst)
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    server.shutdown().await.expect("valid authentication test fixture");
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
    let (server, _) = key_server().await;
    let mut config = create_default_config();
    config.upstream_transport_config.upstream_connection_mode =
        Some(contextforge_data_plane_lib::UpstreamConnectionMode::PlainTextOrTls);
    config.jwks_config.url = server.url("/jwks").parse().expect("valid authentication test fixture");
    let app = gateway(config, store.clone()).await;
    for (role, expected) in [("user", StatusCode::OK), ("unknown", StatusCode::FORBIDDEN)] {
        let mut c = claims();
        c["role"] = role.into();
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
    server.shutdown().await.expect("valid authentication test fixture");
    backend.shutdown().await.expect("valid authentication test fixture");
}
