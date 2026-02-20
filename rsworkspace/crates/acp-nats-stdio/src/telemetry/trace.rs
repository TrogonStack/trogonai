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

    opentelemetry::global::set_tracer_provider(provider.clone());
    TRACER_PROVIDER.set(provider.clone()).ok();

    Ok(provider)
}

pub(super) fn shutdown() {
    if let Some(provider) = TRACER_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        eprintln!("Failed to shutdown tracer provider: {e}");
    }
}
