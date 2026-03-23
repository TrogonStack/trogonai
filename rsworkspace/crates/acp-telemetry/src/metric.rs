use opentelemetry_otlp::MetricExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use std::sync::OnceLock;
use std::time::Duration;

/// OTLP periodic reader pushes accumulated metrics at this cadence.
/// 30 s keeps export volume low while still surfacing near-real-time data.
const METRIC_EXPORT_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();

pub(crate) fn init_provider(
    resource: &Resource,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error>> {
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

pub(crate) fn force_flush() {
    if let Some(provider) = METER_PROVIDER.get()
        && let Err(e) = provider.force_flush()
    {
        eprintln!("Failed to flush meter provider: {e}");
    }
}

pub(crate) fn shutdown() {
    if let Some(provider) = METER_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        eprintln!("Failed to shutdown meter provider: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::Resource;

    #[test]
    fn init_provider_returns_valid_provider() {
        let resource = Resource::builder()
            .with_service_name("test-metric")
            .with_attributes(vec![KeyValue::new("test", "true")])
            .build();

        let provider = init_provider(&resource);
        assert!(provider.is_ok());
    }
}
