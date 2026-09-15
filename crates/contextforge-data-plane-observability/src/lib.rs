//! Shared logging, tracing, metrics, and context propagation for the data plane.

mod config;
mod logging;
mod propagation;

pub use config::{ObservabilityConfig, OtlpProtocol};
pub use logging::{Guard, init_tracing_logging};
pub use propagation::{ExtractingMakeSpan, inject_current_context};
