use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
};
use contextforge_data_plane_apis::{
    User,
    user_store::{BackendMCPGateway, ServiceRoute, UserConfig, VirtualHost},
};
use contextforge_data_plane_lib::{
    AuthorizationClaims, AuthorizationService, Config, ConfigStore, ConfigStoreError, DownstreamTransportConfig,
    Gateway, JwksConfig, ObservabilityConfig, RedisConfig, UpstreamTransportConfig, UserConfigStoreType,
};
use http::{Request, StatusCode, header};
use rmcp::{model::ProtocolVersion, transport::streamable_http_server::session::local::LocalSessionManager};
use serde_json::{Map, Value, json};
use tokio::{runtime::Runtime, sync::Mutex};
use tower::ServiceExt;

const VIRTUAL_HOST_ID: &str = "11111111-1111-1111-1111-111111111111";
const USER_ID: &str = "benchmark-user";
const TARGET_TOOL: &str = "tool-255";
const TARGET_RESOURCE: &str = "benchmark://resource-63";
const RESPONSE_BODY_LIMIT: usize = 64 * 1024;

const BACKEND_COUNT: usize = 4;
const TOOL_COUNT: usize = 256;
const RESOURCE_COUNT: usize = 64;
const PROMPT_COUNT: usize = 32;
const RESOURCE_TEMPLATE_COUNT: usize = 32;

#[derive(Clone, Copy, Debug)]
pub enum Scenario {
    Discover,
    ExcessiveStandardHeaders,
    UnknownTool,
    ParameterHeaderMismatch,
    ToolBackendUnavailable,
    ResourceBackendUnavailable,
}

pub struct BenchmarkRun {
    runtime: Runtime,
    router: Option<Router>,
    request: Option<Request<Body>>,
    response: Option<ResponseSnapshot>,
}

#[derive(Debug)]
pub struct ResponseSnapshot {
    pub status: StatusCode,
    pub body: Bytes,
}

impl BenchmarkRun {
    #[must_use]
    pub fn response(&self) -> &ResponseSnapshot {
        self.response.as_ref().expect("benchmark request has not run")
    }
}

#[derive(Debug)]
struct AlwaysAllowAuthorization {
    claims: AuthorizationClaims,
}

#[async_trait]
impl AuthorizationService for AlwaysAllowAuthorization {
    async fn authorize(&self, _authorization_token: &http::HeaderValue) -> Option<AuthorizationClaims> {
        Some(self.claims.clone())
    }
}

#[derive(Clone)]
struct MemoryUserConfigStore {
    config: Arc<Mutex<UserConfig>>,
}

impl MemoryUserConfigStore {
    fn new(config: UserConfig) -> Self {
        Self { config: Arc::new(Mutex::new(config)) }
    }
}

#[async_trait]
impl ConfigStore<User, UserConfig> for MemoryUserConfigStore {
    async fn get_config<'a>(&self, _key: &'a User) -> Result<UserConfig, ConfigStoreError> {
        Ok(self.config.lock().await.clone())
    }

    async fn set_config<'a>(&self, _key: &'a User, config: &'a UserConfig) -> Result<(), ConfigStoreError> {
        *self.config.lock().await = config.clone();
        Ok(())
    }
}

#[must_use]
pub fn setup(scenario: Scenario) -> BenchmarkRun {
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("benchmark runtime should build");
    let config = benchmark_config();
    let user_config_store = MemoryUserConfigStore::new(benchmark_user_config());
    let authorization_service = AlwaysAllowAuthorization {
        claims: AuthorizationClaims::from(json!({
            "sub": USER_ID,
            "tenant_id": "benchmark-tenant"
        })),
    };
    let router = runtime
        .block_on(
            Gateway::builder()
                .with_config(config)
                .with_session_manager(Arc::new(LocalSessionManager::default()))
                .with_user_config_store_type(UserConfigStoreType::Test(Arc::new(user_config_store)))
                .with_authorization_service(Arc::new(authorization_service))
                .build()
                .into_router(),
        )
        .expect("benchmark router should build");

    BenchmarkRun { runtime, router: Some(router), request: Some(request_for(scenario)), response: None }
}

#[must_use]
pub fn execute(mut benchmark: BenchmarkRun) -> BenchmarkRun {
    let router = benchmark.router.take().expect("benchmark router should be present");
    let request = benchmark.request.take().expect("benchmark request should be present");
    let response = benchmark.runtime.block_on(async move {
        let response = router.oneshot(request).await.expect("benchmark request should complete");
        let status = response.status();
        let body = to_bytes(response.into_body(), RESPONSE_BODY_LIMIT)
            .await
            .expect("benchmark response body should be readable");
        ResponseSnapshot { status, body }
    });
    benchmark.response = Some(response);
    benchmark
}

fn benchmark_config() -> Config {
    Config {
        address: None,
        observability_config: ObservabilityConfig::default(),
        jwks_config: JwksConfig {
            url: "http://127.0.0.1:8080/".parse().expect("benchmark JWKS URL should be valid"),
            ca_cert_path: None,
        },
        user_config_cache_expiry_seconds: 60,
        redis_config: RedisConfig::PlainText { host: String::new(), port: 0 },
        downstream_transport_config: DownstreamTransportConfig::default(),
        upstream_transport_config: UpstreamTransportConfig::default(),
        #[cfg(feature = "with_tools")]
        token_verification_private_key: "assets/jwt.key".into(), // pragma: allowlist secret
        cel_principal_extractor_path: None,
        mcp_allowed_origins: None,
        mcp_allowed_hosts: None,
        mcp_standard_header_max_count: 32,
        mcp_standard_header_max_value_bytes: 8 * 1024,
        mcp_standard_header_max_total_bytes: 64 * 1024,
        runtime_plugins_enabled: None,
    }
}

fn benchmark_user_config() -> UserConfig {
    let mut backends = HashMap::with_capacity(BACKEND_COUNT);
    for index in 0..BACKEND_COUNT {
        let backend_name = backend_name(index);
        let mut tool_schemas = HashMap::new();
        if index == (TOOL_COUNT - 1) % BACKEND_COUNT {
            tool_schemas.insert(TARGET_TOOL.to_owned(), parameter_header_schema());
        }
        backends.insert(
            backend_name.clone(),
            BackendMCPGateway {
                name: backend_name,
                url: if index == (TOOL_COUNT - 1) % BACKEND_COUNT {
                    "benchmark://backend/mcp".parse().expect("benchmark backend URL should be valid")
                } else {
                    "http://127.0.0.1:9/mcp".parse().expect("benchmark backend URL should be valid")
                },
                mcp_protocol_version: ProtocolVersion::V_2026_07_28,
                passthrough_headers: Vec::new(),
                add_headers: HashMap::new(),
                remove_headers: Vec::new(),
                completion: HashMap::new(),
                tool_schemas,
            },
        );
    }

    let virtual_host = VirtualHost {
        backends,
        tools: routes("tool", TOOL_COUNT),
        resources: routes("benchmark://resource", RESOURCE_COUNT),
        resource_templates: routes("benchmark://template", RESOURCE_TEMPLATE_COUNT),
        prompts: routes("prompt", PROMPT_COUNT),
    };
    UserConfig { virtual_hosts: HashMap::from([(VIRTUAL_HOST_ID.to_owned(), virtual_host)]) }
}

fn routes(prefix: &str, count: usize) -> HashMap<String, ServiceRoute> {
    (0..count)
        .map(|index| {
            let name = format!("{prefix}-{index}");
            (name.clone(), ServiceRoute { backend_name: backend_name(index % BACKEND_COUNT), upstream_name: name })
        })
        .collect()
}

fn backend_name(index: usize) -> String {
    format!("backend-{index}")
}

fn parameter_header_schema() -> Map<String, Value> {
    json!({
        "type": "object",
        "properties": {
            "a": { "type": "integer", "x-mcp-header": "A" },
            "b": { "type": "integer", "x-mcp-header": "B" },
            "z": { "type": "string", "x-mcp-header": "Z" }
        }
    })
    .as_object()
    .expect("benchmark tool schema should be an object")
    .clone()
}

fn request_for(scenario: Scenario) -> Request<Body> {
    match scenario {
        Scenario::Discover => mcp_request("server/discover", "benchmark-client", &discover_body()),
        Scenario::ExcessiveStandardHeaders => excessive_standard_headers_request(),
        Scenario::UnknownTool => mcp_request("tools/call", "missing-tool", &tool_call_body("missing-tool")),
        Scenario::ParameterHeaderMismatch => parameter_header_request(http::HeaderValue::from_static("different")),
        Scenario::ToolBackendUnavailable => parameter_header_request(http::HeaderValue::from_static("expected")),
        Scenario::ResourceBackendUnavailable => {
            mcp_request("resources/read", TARGET_RESOURCE, &resource_read_body(TARGET_RESOURCE))
        },
    }
}

fn parameter_header_request(final_header: http::HeaderValue) -> Request<Body> {
    let mut request = mcp_request("tools/call", TARGET_TOOL, &tool_call_body(TARGET_TOOL));
    request.headers_mut().insert("mcp-param-a", http::HeaderValue::from_static("1"));
    request.headers_mut().insert("mcp-param-b", http::HeaderValue::from_static("2"));
    request.headers_mut().insert("mcp-param-z", final_header);
    request
}

fn mcp_request(method: &str, name: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/contextforge-rs/servers/{VIRTUAL_HOST_ID}/mcp"))
        .header(header::AUTHORIZATION, "Bearer benchmark-token")
        .header(header::HOST, "benchmark.local")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .header("mcp-name", name)
        .body(Body::from(serde_json::to_vec(&body).expect("benchmark request body should serialize")))
        .expect("benchmark request should build")
}

fn excessive_standard_headers_request() -> Request<Body> {
    let mut request = mcp_request("server/discover", "benchmark-client", &discover_body());
    for index in 0..32 {
        let name = http::HeaderName::from_bytes(format!("mcp-param-benchmark-{index}").as_bytes())
            .expect("benchmark header name should be valid");
        request.headers_mut().insert(name, http::HeaderValue::from_static("value"));
    }
    request
}

fn discover_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": { "_meta": request_metadata() }
    })
}

fn tool_call_body(name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": { "a": 1, "b": 2, "z": "expected" },
            "_meta": request_metadata()
        }
    })
}

fn resource_read_body(uri: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/read",
        "params": {
            "uri": uri,
            "_meta": request_metadata()
        }
    })
}

fn request_metadata() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "gungraun",
            "version": "0.1.0"
        },
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(scenario: Scenario) -> BenchmarkRun {
        execute(setup(scenario))
    }

    fn response_json(run: &BenchmarkRun) -> Value {
        let body = std::str::from_utf8(&run.response().body).expect("benchmark response should be UTF-8");
        let body = body.strip_prefix("data: ").and_then(|body| body.strip_suffix("\n\n")).unwrap_or(body);
        serde_json::from_str(body).unwrap_or_else(|error| {
            panic!(
                "benchmark response should be JSON: status={} body={:?} error={error}",
                run.response().status,
                String::from_utf8_lossy(&run.response().body)
            )
        })
    }

    #[test]
    fn discover_returns_successful_json_rpc_response() {
        let run = run(Scenario::Discover);

        assert_eq!(
            run.response().status,
            StatusCode::OK,
            "unexpected response: {:?}",
            String::from_utf8_lossy(&run.response().body)
        );
        assert!(response_json(&run).get("result").is_some());
    }

    #[test]
    fn excessive_standard_headers_return_request_header_fields_too_large() {
        let run = run(Scenario::ExcessiveStandardHeaders);

        assert_eq!(run.response().status, StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
        assert_eq!(&run.response().body[..], b"MCP standard header limits exceeded");
    }

    #[test]
    fn unknown_tool_returns_invalid_params_without_contacting_backend() {
        let run = run(Scenario::UnknownTool);
        let response = response_json(&run);

        assert_eq!(
            run.response().status,
            StatusCode::BAD_REQUEST,
            "unexpected response: {:?}",
            String::from_utf8_lossy(&run.response().body)
        );
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(response["error"]["message"], "Routing problem... tool not found");
    }

    #[test]
    fn final_parameter_header_mismatch_returns_error_without_contacting_backend() {
        let run = run(Scenario::ParameterHeaderMismatch);
        let response = response_json(&run);

        assert_eq!(
            run.response().status,
            StatusCode::BAD_REQUEST,
            "unexpected response: {:?}",
            String::from_utf8_lossy(&run.response().body)
        );
        assert_eq!(response["error"]["code"], -32020);
        assert_eq!(response["error"]["message"], "Mcp-Param-Z header `different` does not match body value `expected`");
    }

    #[test]
    fn valid_tool_call_reaches_backend_setup_without_network_io() {
        let run = run(Scenario::ToolBackendUnavailable);
        let response = response_json(&run);

        assert_eq!(
            run.response().status,
            StatusCode::OK,
            "unexpected response: {:?}",
            String::from_utf8_lossy(&run.response().body)
        );
        assert_eq!(response["error"]["code"], -32603);
        assert_eq!(response["error"]["message"], "Routing problem... backend unavailable");
    }

    #[test]
    fn resource_read_reaches_backend_setup_without_network_io() {
        let run = run(Scenario::ResourceBackendUnavailable);
        let response = response_json(&run);

        assert_eq!(
            run.response().status,
            StatusCode::OK,
            "unexpected response: {:?}",
            String::from_utf8_lossy(&run.response().body)
        );
        assert_eq!(response["error"]["code"], -32603);
        assert_eq!(response["error"]["message"], "Routing problem... backend unavailable");
    }
}
