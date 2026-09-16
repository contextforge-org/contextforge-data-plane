use tracing::Subscriber;
use tracing_subscriber::{
    Layer, filter,
    fmt::{self, format::FmtSpan},
    registry::LookupSpan,
};

const DEFAULT_LOGGING: &str = "debug,hyper_util=OFF,tower_http=OFF,rmcp=warn,reqwest=warn,rustls=WARN,h2=WARN,opentelemetry_sdk=WARN,opentelemetry-otlp=WARN";

/// Builds the console logging layer and its `RUST_LOG` filter.
pub(crate) fn layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    let console_filter =
        tracing_subscriber::EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOGGING.to_owned()));

    fmt::layer()
        .event_format(fmt::format().compact())
        .with_target(true)
        .with_span_events(FmtSpan::NONE)
        .with_ansi(false)
        .with_filter(filter::filter_fn(|meta| !meta.is_span()))
        .with_filter(console_filter)
}

pub(crate) fn default_filter() -> &'static str {
    DEFAULT_LOGGING
}
