mod cli_config;
mod config;
use crate::{authorization::AuthorizationService, config_stores::ConfigStore};

pub use contextforge_data_plane_apis::GlobalConfig;
pub use contextforge_data_plane_apis::user_store::UserConfig;
use http::uri::Authority;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use typed_builder::TypedBuilder;

#[allow(unused)]
#[derive(Clone)]
pub struct ContextForgeDataPlaneAppState {
    pub(crate) authorization_service: Arc<dyn AuthorizationService + Send + Sync>,
    pub(crate) config_store: Arc<dyn ConfigStore<contextforge_data_plane_apis::User, UserConfig> + Send + Sync>,
    pub(crate) config: Config,
}

#[derive(Clone, Debug, Serialize, Deserialize, TypedBuilder)]
pub struct Scopes {
    server_id: Option<String>,
    permissions: Vec<String>,
    ip_restrictions: Vec<String>,
    time_restrictions: Option<serde_json::Value>,
}

pub type RedisClient = redis::Client;

pub use cli_config::CliConfig;
pub use config::{
    Config, DEFAULT_MCP_STANDARD_HEADER_MAX_COUNT, DEFAULT_MCP_STANDARD_HEADER_MAX_TOTAL_BYTES,
    DEFAULT_MCP_STANDARD_HEADER_MAX_VALUE_BYTES, DownstreamTransportConfig, JwksConfig, ObservabilityConfig,
    OtlpProtocol, RedisConfig, RedisConnectionMode, UpstreamConnectionMode, UpstreamTransportConfig,
};

impl Config {
    pub fn merge(mut self, rhs: GlobalConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        self.mcp_standard_header_max_count =
            rhs.mcp_standard_header_max_count.unwrap_or(DEFAULT_MCP_STANDARD_HEADER_MAX_COUNT);
        self.mcp_standard_header_max_total_bytes =
            rhs.mcp_standard_header_max_total_bytes.unwrap_or(DEFAULT_MCP_STANDARD_HEADER_MAX_VALUE_BYTES);
        self.mcp_standard_header_max_value_bytes =
            rhs.mcp_standard_header_max_value_bytes.unwrap_or(DEFAULT_MCP_STANDARD_HEADER_MAX_VALUE_BYTES);

        if let Some(hosts) = rhs.mcp_allowed_hosts {
            self.mcp_allowed_hosts =
                Some(hosts.into_iter().map(Authority::try_from).collect::<Result<Vec<Authority>, _>>()?);
        } else {
            self.mcp_allowed_hosts = None;
        }

        self.mcp_allowed_origins = rhs.mcp_allowed_origins;
        Ok(self)
    }
}

impl TryFrom<cli_config::CliConfig> for Config {
    type Error = Box<dyn std::error::Error + Send + Sync>;
    fn try_from(value: cli_config::CliConfig) -> Result<Self, Self::Error> {
        let redis_config = RedisConfig::try_from(&value)?;
        let observability_config = ObservabilityConfig::from(&value);
        let downstream_transport_config = DownstreamTransportConfig::from(&value);
        let upstream_transport_config = UpstreamTransportConfig::from(&value);
        let jwks_config = JwksConfig::from(&value);
        let CliConfig {
            address,
            runtime_plugins_enabled,
            cel_principal_extractor_path,
            #[cfg(feature = "with_tools")]
            token_verification_private_key,
            ..
        } = value;

        Ok(Self {
            address,
            jwks_config,
            observability_config,
            downstream_transport_config,
            upstream_transport_config,
            redis_config,
            runtime_plugins_enabled,
            cel_principal_extractor_path,
            mcp_standard_header_max_count: DEFAULT_MCP_STANDARD_HEADER_MAX_COUNT,
            mcp_standard_header_max_value_bytes: DEFAULT_MCP_STANDARD_HEADER_MAX_VALUE_BYTES,
            mcp_standard_header_max_total_bytes: DEFAULT_MCP_STANDARD_HEADER_MAX_TOTAL_BYTES,
            user_config_cache_expiry_seconds: Duration::from_mins(5).as_secs(),
            mcp_allowed_origins: None,
            mcp_allowed_hosts: None,
            #[cfg(feature = "with_tools")]
            token_verification_private_key,
        })
    }
}
