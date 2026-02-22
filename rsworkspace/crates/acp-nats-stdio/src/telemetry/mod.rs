mod log;
mod metric;
mod trace;

use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_std::env::ReadEnv;
use trogon_std::fs::{CreateDirAll, OpenAppendFile};

const OTEL_SERVICE_NAME: &str = "acp-nats-stdio";

/// File name for the rolling log written to the platform log directory.
const LOG_FILE_NAME: &str = "acp-nats-stdio.log";

fn try_open_log_file<F: CreateDirAll + OpenAppendFile>(env: &impl ReadEnv, fs: &F) -> (Option<std::sync::Mutex<F::Writer>>, Option<String>) {
    let log_dir = match log::ensure_log_dir(env, fs) {
        Ok(dir) => dir,
        Err(e) => return (None, Some(format!("File logging disabled: {}", e))),
    };

    let log_file = log_dir.join(LOG_FILE_NAME);
    match fs.open_append(&log_file) {
        Ok(writer) => (
            Some(std::sync::Mutex::new(writer)),
            Some(format!("File logging enabled: {}", log_file.display())),
        ),
        Err(e) => (
            None,
            Some(format!(
                "Failed to create log file {}: {}",
                log_file.display(),
                e
            )),
        ),
    }
}

pub fn init_logger<E: ReadEnv, F: CreateDirAll + OpenAppendFile>(acp_prefix: &str, env: &E, fs: &F) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE)
        .json();

    let (log_file, file_layer_info) = try_open_log_file(env, fs);
    let file_layer = log_file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .json()
    });

    match try_init_otel(acp_prefix) {
        Ok((tracer_provider, meter_provider, logger_provider)) => {
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
            opentelemetry::global::set_tracer_provider(tracer_provider.clone());

            let tracer = tracer_provider.tracer(OTEL_SERVICE_NAME);
            let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            if trace::TRACER_PROVIDER.set(tracer_provider).is_err() {
                tracing::warn!("Tracer provider already initialized; keeping existing provider");
            }

            opentelemetry::global::set_meter_provider(meter_provider.clone());
            if metric::METER_PROVIDER.set(meter_provider).is_err() {
                tracing::warn!("Meter provider already initialized; keeping existing provider");
            }

            let otel_logs_layer = OpenTelemetryTracingBridge::new(&logger_provider);
            if log::LOGGER_PROVIDER.set(logger_provider).is_err() {
                tracing::warn!("Logger provider already initialized; keeping existing provider");
            }

            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .with(otel_trace_layer)
                .with(otel_logs_layer)
                .init();

            tracing::info!("Logger initialized with OpenTelemetry");
            if let Some(msg) = file_layer_info {
                tracing::info!("{}", msg);
            }
        }
        Err(e) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();

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

fn try_init_otel(
    acp_prefix: &str,
) -> Result<
    (
        opentelemetry_sdk::trace::SdkTracerProvider,
        opentelemetry_sdk::metrics::SdkMeterProvider,
        opentelemetry_sdk::logs::SdkLoggerProvider,
    ),
    Box<dyn std::error::Error>,
> {
    let resource = Resource::builder()
        .with_service_name(OTEL_SERVICE_NAME)
        .with_attributes(vec![KeyValue::new("acp.prefix", acp_prefix.to_owned())])
        .build();

    let tracer_provider = trace::init_provider(&resource)?;
    let meter_provider = metric::init_provider(&resource)?;
    let logger_provider = log::init_provider(&resource)?;

    Ok((tracer_provider, meter_provider, logger_provider))
}

pub fn shutdown_otel() {
    tracing::info!("Shutting down OpenTelemetry providers");

    trace::force_flush();
    metric::force_flush();
    log::force_flush();

    trace::shutdown();
    metric::shutdown();
    log::shutdown();
}
