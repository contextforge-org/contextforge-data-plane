use clap::Parser;

use std::{net::SocketAddr, path::PathBuf};

use crate::common::config::{OtlpProtocol, RedisConnectionMode, UpstreamConnectionMode};

#[derive(Debug, Clone, Parser)]
#[command(name = "contextforge-data-plane")]
#[command(about = "Minimal, fast, experimental data plane for ContextForge")]
pub struct CliConfig {
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_ADDRESS")]
    pub address: Option<SocketAddr>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_JWKS_URL")]
    pub jwks_url: url::Url,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_JWKS_CA_PATH")]
    pub jwks_ca_cert_path: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_ENABLE_OPEN_TELEMETRY")]
    pub enable_open_telemetry: Option<bool>,

    /// OTLP exporter endpoint. For `grpc` this is the collector address
    /// (e.g. `http://127.0.0.1:4317`). For `http-protobuf` this must be the
    /// full traces URL (e.g. `http://langfuse-web:3000/api/public/otel/v1/traces`
    /// for Langfuse, or `http://collector:4318/v1/traces` for the OTel Collector).
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_ENDPOINT")]
    pub otlp_endpoint: Option<http::Uri>,

    /// OTLP wire protocol. Use `http-protobuf` when exporting directly to
    /// Langfuse (it does not accept gRPC).
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_PROTOCOL")]
    pub otlp_protocol: Option<OtlpProtocol>,

    /// Additional headers to attach to every OTLP request, formatted as
    /// `key1=value1,key2=value2`. Used to pass authentication (for example
    /// Langfuse's `Authorization=Basic <base64(public:secret)>`).
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_HEADERS")]
    pub otlp_headers: Option<String>,

    /// Overrides the `service.name` OpenTelemetry resource attribute.
    /// Defaults to `CONTEXTFORGE-DATA-PLANE`.
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_OTEL_SERVICE_NAME")]
    pub otlp_service_name: Option<String>,

    /// Enables OTLP export of HTTP server metrics (request counts, latency
    /// histograms, in-flight gauge, body sizes) emitted by `axum-otel-metrics`.
    /// Independent from `enable_open_telemetry` so traces and metrics can be
    /// turned on individually. Langfuse does not ingest metrics, so this
    /// typically targets an OpenTelemetry Collector.
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_ENABLE_OTEL_METRICS")]
    pub enable_otel_metrics: Option<bool>,

    /// OTLP metrics endpoint. For `grpc` defaults to `http://127.0.0.1:4317`;
    /// for `http-protobuf` defaults to `http://127.0.0.1:4318/v1/metrics`.
    /// Kept separate from `otlp_endpoint` so traces and metrics can be routed
    /// to different backends (typical: traces to Langfuse, metrics to an
    /// OTel Collector).
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")]
    pub otlp_metrics_endpoint: Option<http::Uri>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_RUNTIME_PLUGINS_ENABLED")]
    pub runtime_plugins_enabled: Option<bool>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_TLS_ADDRESS")]
    pub tls_address: Option<SocketAddr>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_TLS_SERVER_PRIVATE_KEY")]
    pub server_private_key: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_TLS_SERVER_CERTIFICATE")]
    pub server_certificate: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_UPSTREAM_CONNECTION_MODE")]
    pub upstream_connection_mode: Option<UpstreamConnectionMode>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_TLS_UPSTREAM_PRIVATE_KEY")]
    pub upstream_private_key: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_TLS_UPSTREAM_CERTIFICATE")]
    pub upstream_certificate: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_TLS_UPSTREAM_TRUST_BUNDLE")]
    pub upstream_trust_bundle: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_REDIS_HOSTNAME")]
    pub redis_address: String,
    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_REDIS_PORT")]
    pub redis_port: u16,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_REDIS_CONNECTION_MODE")]
    pub redis_mode: RedisConnectionMode,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_REDIS_TLS_REDIS_TRUST_BUNDLE")]
    pub redis_tls_trust_bundle: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_REDIS_TLS_REDIS_CLIENT_PRIVATE_KEY")]
    pub redis_tls_client_private_key: Option<PathBuf>,

    #[arg(long, env = "CONTEXTFORGE_DATA_PLANE_REDIS_TLS_REDIS_CLIENT_CERTIFICATE")]
    pub redis_tls_client_certificate: Option<PathBuf>,

    #[cfg(feature = "with_tools")]
    #[arg(long)]
    pub token_verification_private_key: PathBuf,

    #[arg(long)]
    pub cel_principal_extractor_path: Option<PathBuf>,
}
