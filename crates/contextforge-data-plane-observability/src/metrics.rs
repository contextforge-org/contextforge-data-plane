use opentelemetry::global;
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

use crate::{ObservabilityConfig, OtlpProtocol, otlp};

const DEFAULT_GRPC_METRICS_ENDPOINT: &str = "http://127.0.0.1:4317";
const DEFAULT_HTTP_METRICS_ENDPOINT: &str = "http://127.0.0.1:4318/v1/metrics";
const METRICS_EXPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Builds an OTLP metrics pipeline when metric export is enabled.
///
/// Mirrors the trace exporter's gRPC / HTTP protocol branching and reuses
/// the same `service.name` resource attribute so traces and metrics show up
/// under one identity.
///
/// Returns `None` when metrics are disabled; the returned provider must be
/// held alive (via [`crate::ObservabilityRuntime`]) for the [`PeriodicReader`]'s
/// background task to keep exporting.
pub(crate) fn build_meter_provider(
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

    Ok(Some(provider))
}

pub(crate) fn install_global(provider: &SdkMeterProvider) {
    global::set_meter_provider(provider.clone());
}
