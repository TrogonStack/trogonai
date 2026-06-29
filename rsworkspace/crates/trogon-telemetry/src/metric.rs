use opentelemetry_otlp::MetricExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use std::sync::OnceLock;

use crate::TelemetryProviderShutdownError;
use crate::constants::METRIC_EXPORT_INTERVAL;

pub(crate) static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();

pub(crate) fn init_provider(resource: &Resource) -> anyhow::Result<SdkMeterProvider> {
    let exporter = MetricExporter::builder().with_http().build()?;

    let reader = PeriodicReader::builder(exporter)
        .with_interval(METRIC_EXPORT_INTERVAL)
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(crate) fn shutdown() -> Result<(), TelemetryProviderShutdownError> {
    if let Some(provider) = METER_PROVIDER.get()
        && let Err(source) = provider.shutdown()
    {
        return Err(TelemetryProviderShutdownError::Meter { source: source.into() });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
