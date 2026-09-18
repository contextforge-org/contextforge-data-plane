use std::fmt;

pub(crate) const DEFAULT_SERVICE_NAME: &str = "CONTEXTFORGE-DATA-PLANE";

/// Wire protocol used to export OpenTelemetry data.
///
/// `Grpc` targets the standard OTLP/gRPC port, while `HttpProtobuf` sends
/// protobuf payloads over HTTP/1.1 to the signal-specific endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    HttpProtobuf,
}

/// Process-wide console logging, OTLP trace, and OTLP metric configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub traces_enabled: bool,
    pub traces_endpoint: Option<http::Uri>,
    pub metrics_enabled: bool,
    pub metrics_endpoint: Option<http::Uri>,
    pub protocol: OtlpProtocol,
    pub headers: Option<String>,
    pub service_name: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            traces_enabled: false,
            traces_endpoint: None,
            metrics_enabled: false,
            metrics_endpoint: None,
            protocol: OtlpProtocol::default(),
            headers: None,
            service_name: DEFAULT_SERVICE_NAME.to_owned(),
        }
    }
}

impl fmt::Debug for ObservabilityConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObservabilityConfig")
            .field("traces_enabled", &self.traces_enabled)
            .field("traces_endpoint", &self.traces_endpoint)
            .field("metrics_enabled", &self.metrics_enabled)
            .field("metrics_endpoint", &self.metrics_endpoint)
            .field("protocol", &self.protocol)
            .field("headers_configured", &self.headers.is_some())
            .field("service_name", &self.service_name)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SERVICE_NAME, ObservabilityConfig};

    #[test]
    fn defaults_disable_export_and_set_service_name() {
        let config = ObservabilityConfig::default();

        assert!(!config.traces_enabled);
        assert!(!config.metrics_enabled);
        assert_eq!(config.service_name, DEFAULT_SERVICE_NAME);
    }

    #[test]
    fn debug_redacts_headers() {
        let config = ObservabilityConfig { headers: Some("x-test=value".to_owned()), ..Default::default() };

        let debug = format!("{config:?}");
        assert!(debug.contains("headers_configured: true"));
        assert!(!debug.contains("x-test=value"));
    }
}
