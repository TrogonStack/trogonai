pub mod constants;
mod log;
mod metric;
mod service_name;
pub mod signal;
mod trace;

pub use service_name::ServiceName;

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

#[derive(Debug)]
pub struct TelemetryShutdownError {
    errors: Vec<String>,
}

impl std::fmt::Display for TelemetryShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "failed to shutdown OpenTelemetry providers:")?;
        for error in &self.errors {
            writeln!(f, "  - {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TelemetryShutdownError {}

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

pub fn init_logger<E: ReadEnv, F: CreateDirAll + OpenAppendFile>(
    service_name: ServiceName,
    acp_prefix: &str,
    env: &E,
    fs: &F,
) {
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

    match try_init_otel(service_name, acp_prefix) {
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
    service_name: ServiceName,
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
        .with_service_name(service_name.as_str())
        .with_attributes(vec![KeyValue::new("acp.prefix", acp_prefix.to_owned())])
        .build();

    let tracer_provider = trace::init_provider(&resource)?;
    let meter_provider = metric::init_provider(&resource)?;
    let logger_provider = log::init_provider(&resource)?;

    Ok((tracer_provider, meter_provider, logger_provider))
}

pub fn shutdown_otel() -> Result<(), TelemetryShutdownError> {
    tracing::info!("Shutting down OpenTelemetry providers");

    trace::force_flush();
    metric::force_flush();
    log::force_flush();

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
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;
    use trogon_std::fs::MemFs;

    #[test]
    fn try_open_log_file_succeeds_with_env_override() {
        let env = InMemoryEnv::new();
        env.set("ACP_LOG_DIR", "/tmp/test-logs");
        let fs = MemFs::new();

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);
        assert!(writer.is_some());
        let msg = info.unwrap();
        assert!(msg.contains("File logging enabled"));
        assert!(msg.contains("acp-nats-stdio.log"));
    }

    #[test]
    fn try_open_log_file_falls_back_to_platform_dir() {
        let env = InMemoryEnv::new();
        let fs = MemFs::new();

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsWs, &env, &fs);
        assert!(writer.is_some());
        let msg = info.unwrap();
        assert!(msg.contains("File logging enabled"));
    }

    #[test]
    fn try_open_log_file_reports_disabled_when_dir_fails() {
        let env = InMemoryEnv::new();
        let fs = MemFs::new();
        fs.insert("/tmp/test-logs", "file-blocking-dir");
        env.set("ACP_LOG_DIR", "/tmp/test-logs/sub");

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);
        assert!(writer.is_none());
        let msg = info.unwrap();
        assert!(msg.contains("File logging disabled"));
    }

    #[test]
    fn service_name_reexported() {
        assert_eq!(ServiceName::AcpNatsStdio.as_str(), "acp-nats-stdio");
        assert_eq!(ServiceName::AcpNatsWs.as_str(), "acp-nats-ws");
    }

    #[test]
    fn meter_returns_named_meter() {
        let m = meter("coverage-test");
        let counter = m.u64_counter("c").build();
        counter.add(1, &[]);
        assert!(!format!("{:?}", m).is_empty());
    }
}
