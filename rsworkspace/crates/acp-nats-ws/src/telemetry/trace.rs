use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::{Resource, trace as sdktrace};
use std::sync::OnceLock;

pub(super) static TRACER_PROVIDER: OnceLock<sdktrace::SdkTracerProvider> = OnceLock::new();

pub(super) fn init_provider(
    resource: &Resource,
) -> Result<sdktrace::SdkTracerProvider, Box<dyn std::error::Error>> {
    let exporter = SpanExporter::builder().with_http().build()?;

    let provider = sdktrace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(super) fn force_flush() {
    if let Some(provider) = TRACER_PROVIDER.get()
        && let Err(e) = provider.force_flush()
    {
        eprintln!("Failed to flush tracer provider: {e}");
    }
}

pub(super) fn shutdown() {
    if let Some(provider) = TRACER_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        eprintln!("Failed to shutdown tracer provider: {e}");
    }
}
