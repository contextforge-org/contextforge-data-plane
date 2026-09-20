mod auth;
mod client;
mod compatibility;
mod gateway_fixture;
pub(crate) mod mock_counter;
pub(crate) mod paginating_mock;
mod plugin;
mod plugin_gateway;
mod runtime;
mod server;
mod test_gateways;
mod tool;
mod user_config_store;

pub(crate) const TEST_USER_ID: &str = "11111111-1111-1111-1111-111111111111";
pub(crate) const TEST_USER_EMAIL: &str = "admin@example.com";

#[cfg(feature = "with_tools")]
use std::{path::PathBuf, str::FromStr};

pub(crate) use auth::token; // pragma: allowlist secret
pub(crate) use client::{
    CLIENT_CONNECT_TIMEOUT, TEST_POLL_INTERVAL, connect_modern_client, create_client, create_tls_client,
    modern_client_info,
};
pub(crate) use compatibility::connect_client_with_protocol;
use contextforge_data_plane_lib::{
    Config, DownstreamTransportConfig, JwksConfig, ObservabilityConfig, RedisConfig, UpstreamTransportConfig,
};
pub(crate) use gateway_fixture::{GatewayFixture, GatewayTestConfig};
pub(crate) use plugin::{
    POST_DENY_ERROR_CODE, PRE_DENY_ERROR_CODE, PROMPT_ERROR_MESSAGE, PROMPT_POST_DENY_ERROR_CODE, PromptBehavior,
    PromptTestPlugin, REWRITTEN_PROMPT_RESOURCE, REWRITTEN_PROMPT_TEXT, REWRITTEN_PROMPT_TOPIC, REWRITTEN_SUM_A,
    REWRITTEN_SUM_B, TestPlugin, TestPluginFactory,
};
pub(crate) use plugin_gateway::{
    BACKEND_PROMPT_IMAGE, BACKEND_PROMPT_RESOURCE, RunningGateway, start_gateway, start_gateway_with_events,
    start_gateway_with_json_backend_responses, start_gateway_with_parameter_headers,
};
pub(crate) use runtime::{runtime_with_post, runtime_with_pre, runtime_with_pre_and_post, runtime_with_prompt_plugin};
pub(crate) use server::TestServer;
pub(crate) use test_gateways::{
    start_counter_gateway, start_counter_gateway_with_backends, start_legacy_counter_gateway, start_tls_counter_gateway,
};
pub(crate) use tool::{error_code, error_parts, sum_request, text};
pub(crate) use user_config_store::MemoryUserConfigStore;

pub fn create_default_config() -> Config {
    Config {
        address: None,
        jwks_config: JwksConfig {
            url: "http://127.0.0.1:8080/".parse().expect("should work"),
            ca_cert_path: None,
            issuer: "mcpgateway".to_owned(),
            audiences: vec!["mcpgateway-api".to_owned()],
            algorithms: vec![jsonwebtoken::Algorithm::RS256],
            leeway_seconds: 30,
        },
        principal_config: contextforge_data_plane_lib::PrincipalConfig::default(),

        mcp_standard_header_max_count: 10,
        mcp_standard_header_max_value_bytes: 4096,
        mcp_standard_header_max_total_bytes: 4096,
        runtime_plugins_enabled: None,

        user_config_cache_expiry_seconds: 10,
        redis_config: RedisConfig::PlainText { host: String::new(), port: 0 },
        mcp_allowed_origins: None,
        mcp_allowed_hosts: None,
        cel_principal_extractor_path: None,

        #[cfg(feature = "with_tools")]
        token_verification_private_key: PathBuf::from_str("./assets/jwt.key").expect("This should work"),

        observability_config: ObservabilityConfig::default(),
        downstream_transport_config: DownstreamTransportConfig::default(),
        upstream_transport_config: UpstreamTransportConfig::default(),
    }
}
