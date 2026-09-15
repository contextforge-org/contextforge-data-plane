use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::SdkTracerProvider};
use tracing_subscriber::{Registry, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{ObservabilityConfig, logging, metrics, traces};

#[allow(dead_code)]
pub struct Guard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.meter_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

pub fn init_tracing_logging(
    configuration: &ObservabilityConfig,
) -> Result<Guard, Box<dyn std::error::Error + Send + Sync>> {
    let meter_provider = metrics::init_meter_provider(configuration)?;
    let tracer_provider = traces::init_tracer_provider(configuration)?;
    let registry = Registry::default().with(logging::layer());

    if let Some(provider) = tracer_provider.as_ref() {
        registry.with(traces::layer(provider)).init();
    } else {
        registry.init();
    }

    Ok(Guard { tracer_provider, meter_provider })
}
