use opentelemetry_otlp::MetricExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use std::sync::OnceLock;
use std::time::Duration;

pub(super) static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();

pub(super) fn init_provider(
    resource: &Resource,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error>> {
    let exporter = MetricExporter::builder().with_http().build()?;

    let reader = PeriodicReader::builder(exporter)
        .with_interval(Duration::from_secs(30))
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(super) fn shutdown() {
    if let Some(provider) = METER_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        eprintln!("Failed to shutdown meter provider: {e}");
    }
}
