//! Shared logging, tracing, metrics, and context propagation for the data plane.

mod config;
mod logging;
mod metrics;
mod otlp;
mod propagation;
mod runtime;
mod traces;

pub use config::{ObservabilityConfig, OtlpProtocol};
pub use propagation::{
    CORRELATION_ID_HEADER, ExtractingMakeSpan, RequestObservabilityContext, current_request_context,
    inject_current_context, request_context_layer,
};
pub use runtime::{Guard, init_tracing_logging};
