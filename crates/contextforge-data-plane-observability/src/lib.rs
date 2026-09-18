//! Shared logging, tracing, metrics, and context propagation for the data plane.

mod config;
mod logging;
mod metrics;
mod otlp;
mod propagation;
mod runtime;
mod traces;

pub use config::{ObservabilityConfig, OtlpProtocol};
pub use propagation::{ExtractingMakeSpan, inject_current_context};
pub use runtime::{ObservabilityRuntime, ObservabilityShutdownError};
