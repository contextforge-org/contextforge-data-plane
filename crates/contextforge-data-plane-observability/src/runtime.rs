use std::{error::Error, fmt};

use opentelemetry_sdk::{error::OTelSdkError, metrics::SdkMeterProvider, trace::SdkTracerProvider};
use tracing_subscriber::{Registry, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{ObservabilityConfig, logging, metrics, traces};

/// Owns the process-wide OpenTelemetry providers for their full lifetime.
#[must_use = "dropping the runtime immediately shuts down trace and metric export"]
pub struct ObservabilityRuntime {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

/// Errors encountered while flushing and shutting down OpenTelemetry providers.
#[derive(Debug)]
pub struct ObservabilityShutdownError {
    trace: Option<OTelSdkError>,
    metrics: Option<OTelSdkError>,
}

impl fmt::Display for ObservabilityShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.trace, &self.metrics) {
            (Some(trace), Some(metrics)) => {
                write!(formatter, "trace provider shutdown failed: {trace}; metric provider shutdown failed: {metrics}")
            },
            (Some(trace), None) => write!(formatter, "trace provider shutdown failed: {trace}"),
            (None, Some(metrics)) => write!(formatter, "metric provider shutdown failed: {metrics}"),
            (None, None) => formatter.write_str("observability provider shutdown failed"),
        }
    }
}

impl Error for ObservabilityShutdownError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.trace
            .as_ref()
            .map(|error| error as &(dyn Error + 'static))
            .or_else(|| self.metrics.as_ref().map(|error| error as &(dyn Error + 'static)))
    }
}

impl ObservabilityRuntime {
    /// Installs console logging and the configured OTLP trace and metric exporters.
    ///
    /// All fallible provider construction completes before any process-global
    /// state is changed.
    pub fn install(configuration: &ObservabilityConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let runtime = Self::build(configuration)?;
        let registry = Registry::default().with(logging::layer());

        if let Some(provider) = runtime.tracer_provider.as_ref() {
            registry.with(traces::layer(provider)).try_init()?;
        } else {
            registry.try_init()?;
        }

        if runtime.tracer_provider.is_some() {
            traces::install_propagator();
        }
        if let Some(provider) = runtime.meter_provider.as_ref() {
            metrics::install_global(provider);
        }

        Ok(runtime)
    }

    /// Flushes pending spans and metrics and releases exporter resources.
    pub fn shutdown(mut self) -> Result<(), ObservabilityShutdownError> {
        self.shutdown_providers()
    }

    fn build(configuration: &ObservabilityConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let meter_provider = metrics::build_meter_provider(configuration)?;
        let tracer_provider = traces::build_tracer_provider(configuration)?;
        Ok(Self { tracer_provider, meter_provider })
    }

    fn shutdown_providers(&mut self) -> Result<(), ObservabilityShutdownError> {
        let trace = self.tracer_provider.take().and_then(|provider| provider.shutdown().err());
        let metrics = self.meter_provider.take().and_then(|provider| provider.shutdown().err());

        if trace.is_none() && metrics.is_none() { Ok(()) } else { Err(ObservabilityShutdownError { trace, metrics }) }
    }
}

impl Drop for ObservabilityRuntime {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown_providers() {
            tracing::warn!("ObservabilityRuntime::drop - failed to shut down providers error = {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ObservabilityRuntime;
    use crate::{ObservabilityConfig, OtlpProtocol};

    #[tokio::test]
    async fn build_enables_metrics_without_traces() {
        let configuration = ObservabilityConfig {
            traces_enabled: false,
            traces_endpoint: None,
            metrics_enabled: true,
            metrics_endpoint: None,
            protocol: OtlpProtocol::Grpc,
            headers: None,
            service_name: "test-service".to_owned(),
        };

        let runtime = ObservabilityRuntime::build(&configuration).expect("observability runtime should build");

        assert!(runtime.tracer_provider.is_none());
        assert!(runtime.meter_provider.is_some());
        runtime.shutdown().expect("observability runtime should shut down");
    }
}
