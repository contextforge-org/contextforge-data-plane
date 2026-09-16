use opentelemetry::global;
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

use crate::{ObservabilityConfig, OtlpProtocol, otlp};

const DEFAULT_GRPC_METRICS_ENDPOINT: &str = "http://127.0.0.1:4317";
const DEFAULT_HTTP_METRICS_ENDPOINT: &str = "http://127.0.0.1:4318/v1/metrics";
const METRICS_EXPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Builds an OTLP metrics pipeline and installs it as the process-wide
/// [`global::meter_provider`] when `enable_otel_metrics = true`.
///
/// Mirrors the trace exporter's gRPC / HTTP protocol branching and reuses
/// the same `service.name` resource attribute so traces and metrics show up
/// under one identity.
///
/// Returns `None` when metrics are disabled; the returned provider must be
/// held alive (via [`crate::Guard`]) for the [`PeriodicReader`]'s background
/// task to keep exporting.
pub(crate) fn init_meter_provider(
    configuration: &ObservabilityConfig,
) -> Result<Option<SdkMeterProvider>, Box<dyn std::error::Error + Send + Sync>> {
    if !configuration.metrics_enabled {
        return Ok(None);
    }

    let headers = otlp::parse_otlp_headers(configuration.headers.as_deref())?;

    let exporter = match configuration.protocol {
        OtlpProtocol::Grpc => {
            let endpoint = configuration
                .metrics_endpoint
                .as_ref()
                .map_or_else(|| DEFAULT_GRPC_METRICS_ENDPOINT.to_owned(), ToString::to_string);
            MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .with_metadata(otlp::headers_to_metadata(&headers)?)
                .with_timeout(std::time::Duration::from_secs(3))
                .build()?
        },
        OtlpProtocol::HttpProtobuf => {
            let endpoint = configuration
                .metrics_endpoint
                .as_ref()
                .map_or_else(|| DEFAULT_HTTP_METRICS_ENDPOINT.to_owned(), ToString::to_string);
            let mut builder = MetricExporter::builder()
                .with_http()
                .with_endpoint(endpoint)
                .with_protocol(Protocol::HttpBinary)
                .with_timeout(std::time::Duration::from_secs(10));
            if !headers.is_empty() {
                builder = builder.with_headers(headers);
            }
            builder.build()?
        },
    };

    let reader = PeriodicReader::builder(exporter).with_interval(METRICS_EXPORT_INTERVAL).build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_attributes(vec![opentelemetry::KeyValue::new("service.name", configuration.service_name.clone())])
                .build(),
        )
        .build();

    global::set_meter_provider(provider.clone());
    Ok(Some(provider))
}

#[cfg(test)]
mod tests {
    use super::init_meter_provider;
    use crate::{ObservabilityConfig, OtlpProtocol};

    #[tokio::test]
    async fn metrics_initialize_when_trace_export_is_disabled() {
        let configuration = ObservabilityConfig {
            traces_enabled: false,
            traces_endpoint: None,
            metrics_enabled: true,
            metrics_endpoint: None,
            protocol: OtlpProtocol::Grpc,
            headers: None,
            service_name: "test-service".to_owned(),
        };

        let provider = init_meter_provider(&configuration)
            .expect("metrics provider should initialize")
            .expect("metrics should be enabled independently of traces");

        provider.shutdown().expect("metrics provider should shut down");
    }
}
