use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, WithExportConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::constants::{OTLP_ENDPOINT_ENV, SERVICE_VERSION_KEY};
use crate::{ObservabilityError, TracingGuard};

/// Initialize `tracing` with optional OTLP export.
///
/// The endpoint comes from `otlp_endpoint`, falling back to `SANDBOX_OTLP_ENDPOINT`,
/// and is the full OTLP/HTTP traces URL (for example `http://collector:4318/v1/traces`),
/// used as given. Blank values count as unset. Installing the global subscriber is
/// best effort: when one already exists it is left in place and the returned guard
/// exports nothing.
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

    let endpoint = non_blank(otlp_endpoint.map(str::to_owned)).or_else(|| non_blank(env.var(OTLP_ENDPOINT_ENV).ok()));

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
    let installed = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()
        .is_ok();

    Ok(TracingGuard::new(installed.then_some(provider)))
}

fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
