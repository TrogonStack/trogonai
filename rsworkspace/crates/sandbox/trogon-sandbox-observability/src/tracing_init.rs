use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, WithExportConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::{ObservabilityError, TracingGuard};

/// Resource attribute carrying the crate version.
const SERVICE_VERSION_KEY: &str = "service.version";

/// Environment variable consulted when no endpoint is passed explicitly.
pub const OTLP_ENDPOINT_ENV: &str = "SANDBOX_OTLP_ENDPOINT";

/// Initialize `tracing` with optional OTLP export.
///
/// The endpoint comes from `otlp_endpoint`, falling back to `SANDBOX_OTLP_ENDPOINT`.
/// Blank values count as unset. Installing the global subscriber is best effort:
/// when one already exists it is left in place.
pub fn init_tracing(service_name: &str, otlp_endpoint: Option<&str>) -> Result<TracingGuard, ObservabilityError> {
    init_tracing_with_env(service_name, otlp_endpoint, &SystemEnv)
}

/// Same as [`init_tracing`], reading the fallback endpoint from `env`.
pub fn init_tracing_with_env(
    service_name: &str,
    otlp_endpoint: Option<&str>,
    env: &impl ReadEnv,
) -> Result<TracingGuard, ObservabilityError> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer();

    let endpoint = otlp_endpoint
        .map(str::to_owned)
        .or_else(|| env.var(OTLP_ENDPOINT_ENV).ok())
        .map(|endpoint| endpoint.trim().to_owned())
        .filter(|endpoint| !endpoint.is_empty());

    let Some(endpoint) = endpoint else {
        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .try_init();
        return Ok(TracingGuard::new(None));
    };

    let resource = Resource::builder()
        .with_service_name(service_name.to_owned())
        .with_attribute(opentelemetry::KeyValue::new(
            SERVICE_VERSION_KEY,
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpJson)
        .with_endpoint(endpoint)
        .build()
        .map_err(ObservabilityError::Exporter)?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let otel_layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("sandbox"));
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init();

    Ok(TracingGuard::new(Some(provider)))
}
