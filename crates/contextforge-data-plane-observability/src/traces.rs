use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::trace::{RandomIdGenerator, Sampler, SdkTracerProvider};
use tracing::Subscriber;
use tracing_subscriber::{Layer, registry::LookupSpan};

use crate::{ObservabilityConfig, OtlpProtocol, config::DEFAULT_SERVICE_NAME, logging, otlp};

const DEFAULT_GRPC_TRACES_ENDPOINT: &str = "http://127.0.0.1:4317";
const DEFAULT_HTTP_TRACES_ENDPOINT: &str = "http://127.0.0.1:4318/v1/traces";

pub(crate) fn init_tracer_provider(
    configuration: &ObservabilityConfig,
) -> Result<Option<SdkTracerProvider>, Box<dyn std::error::Error + Send + Sync>> {
    if !configuration.traces_enabled {
        return Ok(None);
    }

    let headers = otlp::parse_otlp_headers(configuration.headers.as_deref())?;

    let exporter = match configuration.protocol {
        OtlpProtocol::Grpc => {
            let endpoint = configuration
                .traces_endpoint
                .as_ref()
                .map_or_else(|| DEFAULT_GRPC_TRACES_ENDPOINT.to_owned(), ToString::to_string);
            SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .with_metadata(otlp::headers_to_metadata(&headers)?)
                .with_timeout(std::time::Duration::from_secs(3))
                .build()?
        },
        OtlpProtocol::HttpProtobuf => {
            let endpoint = configuration
                .traces_endpoint
                .as_ref()
                .map_or_else(|| DEFAULT_HTTP_TRACES_ENDPOINT.to_owned(), ToString::to_string);
            let mut builder = SpanExporter::builder()
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

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_id_generator(RandomIdGenerator::default())
        .with_sampler(Sampler::AlwaysOn)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_attributes(vec![opentelemetry::KeyValue::new("service.name", configuration.service_name.clone())])
                .build(),
        )
        .build();

    // Install the W3C propagator so inbound `traceparent` is extracted and
    // outbound requests carry it. Without this, inject/extract are no-ops.
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    Ok(Some(provider))
}

pub(crate) fn layer<S>(provider: &SdkTracerProvider) -> impl Layer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    let tracer = provider.tracer(DEFAULT_SERVICE_NAME);
    let filter = tracing_subscriber::EnvFilter::new(
        std::env::var("RUST_TRACE_LOG").unwrap_or_else(|_| logging::default_filter().to_owned()),
    );

    tracing_opentelemetry::layer().with_tracer(tracer).with_filter(filter)
}
