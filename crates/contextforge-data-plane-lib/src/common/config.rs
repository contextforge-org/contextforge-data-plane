use clap::ValueEnum;
use contextforge_data_plane_observability::{ObservabilityConfig, OtlpProtocol as ObservabilityOtlpProtocol};
use http::uri::Authority;

use redis::{ConnectionAddr, IntoConnectionInfo, RedisError};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer, pem::PemObject};
use url::Url;

use std::{
    fs::{self, File},
    io::{Cursor, Read},
    net::SocketAddr,
    path::PathBuf,
};
use thiserror::Error;

use crate::{CliConfig, RedisClient};

#[derive(Clone)]
pub enum RedisConfig {
    PlainText { host: String, port: u16 },
    Tls { host: String, port: u16, trust_bundle: Vec<u8> },
    MTls { host: String, port: u16, trust_bundle: Vec<u8>, client_cert: Vec<u8>, client_key: Vec<u8> },
}
impl std::fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlainText { host, port } => {
                f.debug_struct("PlainText").field("host", host).field("port", port).finish()
            },
            Self::Tls { host, port, trust_bundle: _ } => {
                f.debug_struct("Tls").field("host", host).field("port", port).finish()
            },
            Self::MTls { host, port, .. } => f.debug_struct("MTls").field("host", host).field("port", port).finish(),
        }
    }
}

impl TryFrom<RedisConfig> for RedisClient {
    type Error = RedisError;

    fn try_from(redis_config: RedisConfig) -> Result<Self, Self::Error> {
        match redis_config {
            RedisConfig::PlainText { host, port } => {
                Ok(RedisClient::open(ConnectionAddr::Tcp(host, port).into_connection_info()?)?)
            },
            RedisConfig::Tls { host, port, trust_bundle } => RedisClient::build_with_tls(
                format!("rediss://{host}:{port}"),
                redis::TlsCertificates { client_tls: None, root_cert: Some(trust_bundle) },
            ),
            RedisConfig::MTls { host, port, trust_bundle, client_cert, client_key } => RedisClient::build_with_tls(
                format!("rediss://{host}:{port}"),
                redis::TlsCertificates {
                    client_tls: Some(redis::ClientTlsConfig { client_cert, client_key }),
                    root_cert: Some(trust_bundle),
                },
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum UpstreamConnectionMode {
    PlainTextOrTls,
    PlainTextOrMTls,
    TlsOnly,
    MtlsOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[derive(Default)]
pub enum RedisConnectionMode {
    PlainText,
    #[default]
    Tls,
    Mtls,
}

/// Wire protocol used to export OpenTelemetry data to the collector / backend.
///
/// `Grpc` targets the standard OTLP/gRPC port (e.g. `4317`) used by
/// collectors such as the OpenTelemetry Collector and Tempo.
/// `HttpProtobuf` targets OTLP over HTTP/1.1 with a protobuf payload
/// (e.g. `4318/v1/traces`) and is the only protocol supported by Langfuse's
/// OTel ingestion endpoint (`/api/public/otel/v1/traces`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Default)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    HttpProtobuf,
}

impl From<&CliConfig> for ObservabilityConfig {
    fn from(value: &CliConfig) -> Self {
        let defaults = ObservabilityConfig::default();
        Self {
            traces_enabled: value.enable_open_telemetry == Some(true),
            traces_endpoint: value.otlp_endpoint.clone(),
            metrics_enabled: value.enable_otel_metrics == Some(true),
            metrics_endpoint: value.otlp_metrics_endpoint.clone(),
            protocol: match value.otlp_protocol.as_ref() {
                None => defaults.protocol,
                Some(OtlpProtocol::Grpc) => ObservabilityOtlpProtocol::Grpc,
                Some(OtlpProtocol::HttpProtobuf) => ObservabilityOtlpProtocol::HttpProtobuf,
            },
            headers: value.otlp_headers.clone(),
            service_name: value.otlp_service_name.clone().unwrap_or(defaults.service_name),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DownstreamTransportConfig {
    pub tls_address: Option<SocketAddr>,
    pub server_private_key: Option<PathBuf>,
    pub server_certificate: Option<PathBuf>,
}
impl From<&CliConfig> for DownstreamTransportConfig {
    fn from(value: &CliConfig) -> Self {
        let CliConfig { tls_address, server_private_key, server_certificate, .. } = value.clone();
        Self { tls_address, server_private_key, server_certificate }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UpstreamTransportConfig {
    pub upstream_connection_mode: Option<UpstreamConnectionMode>,
    pub upstream_private_key: Option<PathBuf>,
    pub upstream_certificate: Option<PathBuf>,
    pub upstream_trust_bundle: Option<PathBuf>,
}
impl From<&CliConfig> for UpstreamTransportConfig {
    fn from(value: &CliConfig) -> Self {
        let CliConfig {
            upstream_connection_mode,
            upstream_private_key,
            upstream_certificate,
            upstream_trust_bundle,
            ..
        } = value.clone();
        Self { upstream_connection_mode, upstream_private_key, upstream_certificate, upstream_trust_bundle }
    }
}

#[derive(Clone, Debug)]
pub struct JwksConfig {
    pub url: url::Url,
    pub ca_cert_path: Option<PathBuf>,
}
impl From<&CliConfig> for JwksConfig {
    fn from(value: &CliConfig) -> Self {
        let CliConfig { jwks_url, jwks_ca_cert_path, .. } = value.clone();
        Self { url: jwks_url, ca_cert_path: jwks_ca_cert_path }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub address: Option<SocketAddr>,

    pub observability_config: ObservabilityConfig,
    pub jwks_config: JwksConfig,

    /// Expiry in seconds for the in-process user config cache in front of
    /// Redis. The control-plane dataplane publisher rewrites UserConfig keys
    /// every 60s, so this bounds how stale a subject's config can get.
    /// 0 disables caching and reads Redis on every request.
    pub user_config_cache_expiry_seconds: u64,

    pub redis_config: RedisConfig,

    pub downstream_transport_config: DownstreamTransportConfig,
    pub upstream_transport_config: UpstreamTransportConfig,

    #[cfg(feature = "with_tools")]
    pub token_verification_private_key: PathBuf,

    pub cel_principal_extractor_path: Option<PathBuf>,

    pub mcp_allowed_origins: Option<Vec<Url>>,

    pub mcp_allowed_hosts: Option<Vec<Authority>>,

    /// Maximum number of MCP standard headers accepted on a single request.
    pub mcp_standard_header_max_count: usize,

    /// Maximum byte length accepted for a single MCP standard header value.
    pub mcp_standard_header_max_value_bytes: usize,

    /// Approximate request-level aggregate bytes accepted across all matched
    /// MCP standard header names and values.
    pub mcp_standard_header_max_total_bytes: usize,

    pub runtime_plugins_enabled: Option<bool>,
}

pub const DEFAULT_MCP_STANDARD_HEADER_MAX_COUNT: usize = 32;
pub const DEFAULT_MCP_STANDARD_HEADER_MAX_VALUE_BYTES: usize = 8 * 1024;
pub const DEFAULT_MCP_STANDARD_HEADER_MAX_TOTAL_BYTES: usize = 64 * 1024;

#[derive(Error, Debug)]
pub enum ConfigValidationError {
    #[error("Redis Configuration Error")]
    RedisConfigurationError(String),
}

impl TryFrom<&CliConfig> for RedisConfig {
    fn try_from(value: &CliConfig) -> Result<Self, Self::Error> {
        let _: Authority = format!("{}:{}", value.redis_address, value.redis_port)
            .parse::<Authority>()
            .map_err(|e| ConfigValidationError::RedisConfigurationError(e.to_string()))?;

        match value.redis_mode {
            RedisConnectionMode::PlainText => {
                Ok(Self::PlainText { host: value.redis_address.clone(), port: value.redis_port })
            },
            RedisConnectionMode::Tls => {
                let Some(trust_bundle) = &value.redis_tls_trust_bundle else {
                    return Err(ConfigValidationError::RedisConfigurationError(format!(
                        "Trust bundle is required for Redis {:?}",
                        value.redis_mode
                    )));
                };

                let trust_bundle = validate_certs(trust_bundle)?;

                Ok(Self::Tls { host: value.redis_address.clone(), port: value.redis_port, trust_bundle })
            },
            RedisConnectionMode::Mtls => {
                let Some(trust_bundle) = &value.redis_tls_trust_bundle else {
                    return Err(ConfigValidationError::RedisConfigurationError(format!(
                        "Trust bundle is required for Redis {:?}",
                        value.redis_mode
                    )));
                };

                let trust_bundle = validate_certs(trust_bundle)?;

                let Some(certificate) = &value.redis_tls_client_certificate else {
                    return Err(ConfigValidationError::RedisConfigurationError(format!(
                        "Client certificate is required for Redis {:?}",
                        value.redis_mode
                    )));
                };

                let client_cert = validate_certs(certificate)?;

                let Some(key) = &value.redis_tls_client_private_key else {
                    return Err(ConfigValidationError::RedisConfigurationError(format!(
                        "Client key is required for Redis {:?}",
                        value.redis_mode
                    )));
                };

                let client_key = validate_key(key)?;

                Ok(Self::MTls {
                    host: value.redis_address.clone(),
                    port: value.redis_port,
                    trust_bundle,
                    client_cert,
                    client_key,
                })
            },
        }
    }

    type Error = ConfigValidationError;
}

fn validate_certs(path: &PathBuf) -> Result<Vec<u8>, ConfigValidationError> {
    let mut buf = Vec::new();
    File::open(path)
        .map_err(|e| ConfigValidationError::RedisConfigurationError(e.to_string()))?
        .read_to_end(&mut buf)
        .map_err(|e| ConfigValidationError::RedisConfigurationError(e.to_string()))?;
    let mut cursor = Cursor::new(buf);

    let certs = CertificateDer::pem_reader_iter(&mut cursor)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ConfigValidationError::RedisConfigurationError(e.to_string()))?;

    if certs.is_empty() {
        Err(ConfigValidationError::RedisConfigurationError("No certificates provided".to_owned()))
    } else {
        Ok(cursor.into_inner())
    }
}

fn validate_key(path: &PathBuf) -> Result<Vec<u8>, ConfigValidationError> {
    let mut buf = Vec::new();
    File::open(path)
        .map_err(|e| ConfigValidationError::RedisConfigurationError(e.to_string()))?
        .read_to_end(&mut buf)
        .map_err(|e| ConfigValidationError::RedisConfigurationError(e.to_string()))?;

    let _ = PrivatePkcs8KeyDer::from_pem_slice(&buf)
        .map_err(|_| ConfigValidationError::RedisConfigurationError("Private key is invalid".to_owned()))?;
    Ok(buf)
}

impl TryFrom<&UpstreamTransportConfig> for reqwest::Client {
    type Error = crate::Error;

    fn try_from(config: &UpstreamTransportConfig) -> Result<Self, Self::Error> {
        let builder = reqwest::Client::builder();
        let builder = match config.upstream_connection_mode.as_ref() {
            None | Some(UpstreamConnectionMode::TlsOnly) => builder.https_only(true),
            Some(UpstreamConnectionMode::PlainTextOrTls) => builder.https_only(false),
            Some(UpstreamConnectionMode::PlainTextOrMTls) => {
                builder.https_only(false).identity(extract_identity(config)?)
            },
            Some(UpstreamConnectionMode::MtlsOnly) => builder.https_only(true).identity(extract_identity(config)?),
        };

        let builder = if let Some(trust_bundle) = config.upstream_trust_bundle.as_ref() {
            let mut buf = Vec::new();
            File::open(trust_bundle)?.read_to_end(&mut buf)?;
            let certificates = reqwest::Certificate::from_pem_bundle(&buf)?;
            builder.tls_certs_merge(certificates)
        } else {
            builder
        };

        Ok(builder.build()?)
    }
}

fn extract_identity(config: &UpstreamTransportConfig) -> crate::Result<reqwest::Identity> {
    match (config.upstream_private_key.as_ref(), config.upstream_certificate.as_ref()) {
        (Some(private_key), Some(certificate)) => {
            let mut cert = fs::read(certificate)?;
            let key = fs::read(private_key)?;
            cert.extend(key);
            Ok(reqwest::Identity::from_pem(&cert)?)
        },

        _ => Err("Invalid/missing configuration".into()),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "with_tools")]
    use std::{path::PathBuf, str::FromStr};

    use clap::Parser;
    use contextforge_data_plane_observability::{ObservabilityConfig, OtlpProtocol as ObservabilityOtlpProtocol};

    use crate::CliConfig;
    use crate::common::config::{DownstreamTransportConfig, UpstreamTransportConfig};

    #[test]
    fn observability_config_is_derived_from_cli_config() {
        let args = vec![
            "contextforge-data-plane",
            "--jwks-url",
            "http://127.0.0.1:8080/",
            "--redis-address",
            "127.0.0.1",
            "--redis-port",
            "6379",
            "--redis-mode",
            "plain-text",
            "--enable-open-telemetry",
            "true",
            "--otlp-endpoint",
            "http://collector:4318/v1/traces",
            "--enable-otel-metrics",
            "true",
            "--otlp-metrics-endpoint",
            "http://collector:4318/v1/metrics",
            "--otlp-protocol",
            "http-protobuf",
            "--otlp-headers",
            "x-test=value",
            "--otlp-service-name",
            "test-service",
        ];
        #[cfg(feature = "with_tools")]
        let args = args.into_iter().chain(["--token-verification-private-key", "./assets/jwt.key"]).collect::<Vec<_>>();

        let cli_config = CliConfig::try_parse_from(args).expect("CLI configuration should parse");
        let config = ObservabilityConfig::from(&cli_config);

        assert!(config.traces_enabled);
        assert_eq!(
            config.traces_endpoint.as_ref().map(ToString::to_string).as_deref(),
            Some("http://collector:4318/v1/traces")
        );
        assert!(config.metrics_enabled);
        assert_eq!(
            config.metrics_endpoint.as_ref().map(ToString::to_string).as_deref(),
            Some("http://collector:4318/v1/metrics")
        );
        assert_eq!(config.protocol, ObservabilityOtlpProtocol::HttpProtobuf);
        assert_eq!(config.headers.as_deref(), Some("x-test=value"));
        assert_eq!(config.service_name, "test-service");
    }

    impl Default for super::JwksConfig {
        fn default() -> Self {
            Self { url: "http://127.0.0.1:8080/".parse().expect("should work"), ca_cert_path: None }
        }
    }

    impl Default for super::Config {
        fn default() -> Self {
            Self {
                address: None,
                jwks_config: super::JwksConfig::default(),
                observability_config: super::ObservabilityConfig::default(),
                mcp_standard_header_max_count: 10,
                mcp_standard_header_max_value_bytes: 4096,
                mcp_standard_header_max_total_bytes: 4096,
                runtime_plugins_enabled: None,

                user_config_cache_expiry_seconds: 10,
                redis_config: super::RedisConfig::PlainText { host: String::new(), port: 0 },
                mcp_allowed_origins: None,
                mcp_allowed_hosts: None,
                cel_principal_extractor_path: None,

                #[cfg(feature = "with_tools")]
                token_verification_private_key: PathBuf::from_str("./assets/jwt.key").expect("This should work"),
                downstream_transport_config: DownstreamTransportConfig::default(),
                upstream_transport_config: UpstreamTransportConfig::default(),
            }
        }
    }
}
