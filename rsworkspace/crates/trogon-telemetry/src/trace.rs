use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::{Resource, trace as sdktrace};
use std::sync::OnceLock;

use crate::TelemetryProviderShutdownError;

pub(crate) static TRACER_PROVIDER: OnceLock<sdktrace::SdkTracerProvider> = OnceLock::new();

pub(crate) fn init_provider(resource: &Resource) -> anyhow::Result<sdktrace::SdkTracerProvider> {
    let exporter = SpanExporter::builder().with_http().build()?;

    let provider = sdktrace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(crate) fn shutdown() -> Result<(), TelemetryProviderShutdownError> {
    if let Some(provider) = TRACER_PROVIDER.get()
        && let Err(source) = provider.shutdown()
    {
        return Err(TelemetryProviderShutdownError::Tracer { source: source.into() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::Resource;

    #[test]
    fn init_provider_returns_valid_provider() {
        let resource = Resource::builder()
            .with_service_name("test-trace")
            .with_attributes(vec![KeyValue::new("test", "true")])
            .build();

        let provider = init_provider(&resource);
        assert!(provider.is_ok());
    }
}
