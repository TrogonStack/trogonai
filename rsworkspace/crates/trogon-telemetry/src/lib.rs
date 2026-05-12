#![cfg_attr(coverage, feature(coverage_attribute))]

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

#[cfg_attr(coverage, coverage(off))]
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

fn try_init_otel<A>(
    service_name: ServiceName,
    resource_attributes: A,
) -> Result<
    (
        opentelemetry_sdk::trace::SdkTracerProvider,
        opentelemetry_sdk::metrics::SdkMeterProvider,
        opentelemetry_sdk::logs::SdkLoggerProvider,
    ),
    Box<dyn std::error::Error>,
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
mod tests {
    use super::*;
    use std::io;
    use std::path::Path;
    use trogon_std::env::InMemoryEnv;
    use trogon_std::fs::{MemAppendWriter, MemFs};

    struct OpenAppendErrorFs {
        inner: MemFs,
    }

    impl OpenAppendErrorFs {
        fn new() -> Self {
            Self { inner: MemFs::new() }
        }
    }

    impl CreateDirAll for OpenAppendErrorFs {
        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            self.inner.create_dir_all(path)
        }
    }

    impl OpenAppendFile for OpenAppendErrorFs {
        type Writer = MemAppendWriter;

        fn open_append(&self, _path: &Path) -> io::Result<Self::Writer> {
            Err(io::Error::other("open append failed"))
        }
    }

    #[test]
    fn telemetry_shutdown_error_formats_all_errors() {
        let error = TelemetryShutdownError {
            errors: vec!["trace failed".to_string(), "metric failed".to_string()],
        };

        let message = error.to_string();

        assert!(message.contains("failed to shutdown OpenTelemetry providers"));
        assert!(message.contains("trace failed"));
        assert!(message.contains("metric failed"));
    }

    #[test]
    fn try_open_log_file_succeeds_with_env_override() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/tmp/test-logs");
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

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsServer, &env, &fs);
        assert!(writer.is_some());
        let msg = info.unwrap();
        assert!(msg.contains("File logging enabled"));
    }

    #[test]
    fn try_open_log_file_reports_disabled_when_dir_fails() {
        let env = InMemoryEnv::new();
        let fs = MemFs::new();
        fs.insert("/tmp/test-logs", "file-blocking-dir");
        env.set("TROGON_LOG_DIR", "/tmp/test-logs/sub");

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);
        assert!(writer.is_none());
        let msg = info.unwrap();
        assert!(msg.contains("File logging disabled"));
    }

    /// Covers the `Err(e)` arm in `try_open_log_file` when `open_append` fails.
    #[test]
    fn try_open_log_file_reports_failed_to_create_when_open_append_fails() {
        use std::io;
        use std::path::Path;
        use trogon_std::fs::CreateDirAll;

        /// A filesystem stub whose `open_append` always returns an I/O error.
        struct FailOpenFs(MemFs);

        impl CreateDirAll for FailOpenFs {
            fn create_dir_all(&self, path: &Path) -> io::Result<()> {
                self.0.create_dir_all(path)
            }
        }

        impl trogon_std::fs::OpenAppendFile for FailOpenFs {
            type Writer = <MemFs as trogon_std::fs::OpenAppendFile>::Writer;
            fn open_append(&self, _path: &Path) -> io::Result<Self::Writer> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            }
        }

        let env = InMemoryEnv::new();
        env.set("ACP_LOG_DIR", "/tmp/test-logs-failopen");
        let fs = FailOpenFs(MemFs::new());

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);
        assert!(writer.is_none());
        let msg = info.unwrap();
        assert!(msg.contains("Failed to create log file"), "got: {msg}");
    }

    #[test]
    fn otel_disabled_when_endpoint_not_set() {
        let env = InMemoryEnv::new();
        assert!(!otel_endpoint_configured(&env));
    }

    #[test]
    fn otel_enabled_when_endpoint_set() {
        let env = InMemoryEnv::new();
        env.set("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318");
        assert!(otel_endpoint_configured(&env));
    }

    #[test]
    fn try_open_log_file_reports_open_append_error() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/tmp/test-logs");
        let fs = OpenAppendErrorFs::new();

        let (writer, info) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);

        assert!(writer.is_none());
        let msg = info.unwrap();
        assert!(msg.contains("Failed to create log file"));
        assert!(msg.contains("open append failed"));
    }

    #[test]
    fn service_name_reexported() {
        assert_eq!(ServiceName::AcpNatsStdio.as_str(), "acp-nats-stdio");
        assert_eq!(ServiceName::AcpNatsServer.as_str(), "acp-nats-server");
    }

    #[test]
    fn meter_returns_named_meter() {
        let m = meter("coverage-test");
        let counter = m.u64_counter("c").build();
        counter.add(1, &[]);
        assert!(!format!("{:?}", m).is_empty());
    }
}
