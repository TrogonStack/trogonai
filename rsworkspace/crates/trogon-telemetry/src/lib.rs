#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod constants;

mod log;
mod metric;
mod resource_attribute;
mod service_name;
mod trace;

pub use resource_attribute::ResourceAttribute;
pub use service_name::ServiceName;

use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use std::error::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_std::env::ReadEnv;
use trogon_std::fs::{CreateDirAll, OpenAppendFile};

#[derive(Debug, thiserror::Error)]
#[error("{}", self.fmt_errors())]
pub struct TelemetryShutdownError {
    errors: Vec<TelemetryProviderShutdownError>,
}

impl TelemetryShutdownError {
    fn fmt_errors(&self) -> String {
        let mut out = String::from("failed to shutdown OpenTelemetry providers:\n");
        for error in &self.errors {
            out.push_str(&format!("  - {error}"));
            if let Some(source) = error.source() {
                out.push_str(&format!(": {source}"));
            }
            out.push('\n');
        }
        out
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TelemetryProviderShutdownError {
    #[error("failed to shutdown tracer provider")]
    Tracer {
        #[source]
        source: anyhow::Error,
    },
    #[error("failed to shutdown meter provider")]
    Meter {
        #[source]
        source: anyhow::Error,
    },
    #[error("failed to shutdown logger provider")]
    Logger {
        #[source]
        source: anyhow::Error,
    },
}

fn try_open_log_file<F: CreateDirAll + OpenAppendFile>(
    service_name: ServiceName,
    env: &impl ReadEnv,
    fs: &F,
) -> (Option<std::sync::Mutex<F::Writer>>, Option<String>) {
    let log_dir = match log::ensure_log_dir(service_name, env, fs) {
        Ok(dir) => dir,
        Err(e) => return (None, Some(format!("File logging disabled: {}", e))),
    };

    let log_file_name = format!("{}.log", service_name.as_str());
    let log_file = log_dir.join(&log_file_name);
    match fs.open_append(&log_file) {
        Ok(writer) => (
            Some(std::sync::Mutex::new(writer)),
            Some(format!("File logging enabled: {}", log_file.display())),
        ),
        Err(e) => (
            None,
            Some(format!("Failed to create log file {}: {}", log_file.display(), e)),
        ),
    }
}

pub fn init_logger<E, F, A>(service_name: ServiceName, resource_attributes: A, env: &E, fs: &F)
where
    E: ReadEnv,
    F: CreateDirAll + OpenAppendFile,
    A: IntoIterator<Item = ResourceAttribute>,
{
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE)
        .json();

    let (log_file, file_layer_info) = try_open_log_file(service_name, env, fs);
    let file_layer = log_file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .json()
    });

    match try_init_otel(service_name, resource_attributes) {
        Ok((tracer_provider, meter_provider, logger_provider)) => {
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            opentelemetry::global::set_tracer_provider(tracer_provider.clone());

            let tracer = tracer_provider.tracer(service_name.as_str().to_owned());
            let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            if trace::TRACER_PROVIDER.set(tracer_provider).is_err() {
                eprintln!("WARN: tracer provider already initialized");
            }

            opentelemetry::global::set_meter_provider(meter_provider.clone());
            if metric::METER_PROVIDER.set(meter_provider).is_err() {
                eprintln!("WARN: meter provider already initialized");
            }

            let otel_logs_layer = OpenTelemetryTracingBridge::new(&logger_provider);
            if log::LOGGER_PROVIDER.set(logger_provider).is_err() {
                eprintln!("WARN: logger provider already initialized");
            }

            if let Err(e) = tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .with(otel_trace_layer)
                .with(otel_logs_layer)
                .try_init()
            {
                eprintln!("WARN: tracing subscriber already initialized: {e}");
            }

            tracing::info!("Logger initialized with OpenTelemetry");
            if let Some(msg) = file_layer_info {
                tracing::info!("{}", msg);
            }
        }
        Err(e) => {
            if let Err(init_error) = tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .try_init()
            {
                eprintln!("WARN: tracing subscriber already initialized: {init_error}");
            }

            tracing::warn!(
                error = %e,
                "Logger initialized without OpenTelemetry (init failed)"
            );
            if let Some(msg) = file_layer_info {
                tracing::info!("{}", msg);
            }
        }
    }
}

/// Initialize OpenTelemetry providers + file logging for an **interactive** host
/// (the `trogon` CLI/REPL) WITHOUT attaching a stderr logging layer.
///
/// Services use [`init_logger`], which writes structured JSON logs to stderr — that
/// is correct for a daemon whose stderr *is* its log stream, but it would corrupt an
/// interactive TUI. The CLI still must initialize the provider so kernel/switch
/// metrics actually export instead of registering against a no-op meter (ADR-0008 /
/// cambio-modelo.md Exit Criteria: "el binario host debe inicializar el provider OTel
/// para que la metrica realmente se exporte"). Logs go to the per-service log file only.
pub fn init_logger_file_only<E, F, A>(service_name: ServiceName, resource_attributes: A, env: &E, fs: &F)
where
    E: ReadEnv,
    F: CreateDirAll + OpenAppendFile,
    A: IntoIterator<Item = ResourceAttribute>,
{
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let (log_file, file_layer_info) = try_open_log_file(service_name, env, fs);
    let file_layer = log_file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .json()
    });

    match try_init_otel(service_name, resource_attributes) {
        Ok((tracer_provider, meter_provider, logger_provider)) => {
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            opentelemetry::global::set_tracer_provider(tracer_provider.clone());

            let tracer = tracer_provider.tracer(service_name.as_str().to_owned());
            let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            if trace::TRACER_PROVIDER.set(tracer_provider).is_err() {
                eprintln!("WARN: tracer provider already initialized");
            }

            opentelemetry::global::set_meter_provider(meter_provider.clone());
            if metric::METER_PROVIDER.set(meter_provider).is_err() {
                eprintln!("WARN: meter provider already initialized");
            }

            let otel_logs_layer = OpenTelemetryTracingBridge::new(&logger_provider);
            if log::LOGGER_PROVIDER.set(logger_provider).is_err() {
                eprintln!("WARN: logger provider already initialized");
            }

            if let Err(e) = tracing_subscriber::registry()
                .with(env_filter)
                .with(file_layer)
                .with(otel_trace_layer)
                .with(otel_logs_layer)
                .try_init()
            {
                eprintln!("WARN: tracing subscriber already initialized: {e}");
            }

            tracing::info!("CLI telemetry initialized with OpenTelemetry (file-only logging)");
            if let Some(msg) = file_layer_info {
                tracing::info!("{}", msg);
            }
        }
        Err(e) => {
            if let Err(init_error) = tracing_subscriber::registry()
                .with(env_filter)
                .with(file_layer)
                .try_init()
            {
                eprintln!("WARN: tracing subscriber already initialized: {init_error}");
            }

            tracing::warn!(
                error = %e,
                "CLI telemetry initialized without OpenTelemetry (init failed)"
            );
            if let Some(msg) = file_layer_info {
                tracing::info!("{}", msg);
            }
        }
    }
}

fn try_init_otel<A>(
    service_name: ServiceName,
    resource_attributes: A,
) -> Result<
    (
        opentelemetry_sdk::trace::SdkTracerProvider,
        opentelemetry_sdk::metrics::SdkMeterProvider,
        opentelemetry_sdk::logs::SdkLoggerProvider,
    ),
    anyhow::Error,
>
where
    A: IntoIterator<Item = ResourceAttribute>,
{
    let resource = Resource::builder()
        .with_service_name(service_name.as_str())
        .with_attributes(resource_attributes.into_iter().map(opentelemetry::KeyValue::from))
        .build();

    let tracer_provider = trace::init_provider(&resource)?;
    let meter_provider = metric::init_provider(&resource)?;
    let logger_provider = log::init_provider(&resource)?;

    Ok((tracer_provider, meter_provider, logger_provider))
}

#[cfg_attr(coverage, coverage(off))]
pub fn shutdown_otel() -> Result<(), TelemetryShutdownError> {
    tracing::info!("Shutting down OpenTelemetry providers");

    let mut errors = Vec::new();
    if let Err(e) = trace::shutdown() {
        errors.push(e);
    }
    if let Err(e) = metric::shutdown() {
        errors.push(e);
    }
    if let Err(e) = log::shutdown() {
        errors.push(e);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(TelemetryShutdownError { errors })
    }
}

pub fn meter(name: &'static str) -> opentelemetry::metrics::Meter {
    opentelemetry::global::meter(name)
}

#[cfg(test)]
mod tests;
