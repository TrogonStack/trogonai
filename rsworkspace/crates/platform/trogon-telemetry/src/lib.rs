#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod constants;
mod file_logging_outcome;
mod log;
mod metric;
mod resource;
mod resource_attribute;
mod service_name;
mod trace;

pub use resource_attribute::ResourceAttribute;
pub use service_name::ServiceName;

use file_logging_outcome::FileLoggingOutcome;
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use std::error::Error;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_std::env::ReadEnv;
use trogon_std::fs::{CreateDirAll, OpenAppendFile};

static LOGGER_INITIALIZED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

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

impl TelemetryProviderShutdownError {
    fn timed_out(index: usize, timeout: Duration) -> Self {
        let source = anyhow::anyhow!("did not finish within {timeout:?}");
        match index {
            0 => Self::Tracer { source },
            1 => Self::Meter { source },
            _ => Self::Logger { source },
        }
    }
}

fn try_open_log_file<F: CreateDirAll + OpenAppendFile>(
    service_name: ServiceName,
    env: &impl ReadEnv,
    fs: &F,
) -> (Option<std::sync::Mutex<F::Writer>>, FileLoggingOutcome) {
    let log_dir = match log::ensure_log_dir(service_name, env, fs) {
        Ok(dir) => dir,
        Err(error) => return (None, FileLoggingOutcome::DirectoryUnavailable { error }),
    };

    let log_file_name = format!("{}.log", service_name.as_str());
    let log_file = log_dir.join(&log_file_name);
    match fs.open_append(&log_file) {
        Ok(writer) => (
            Some(std::sync::Mutex::new(writer)),
            FileLoggingOutcome::Enabled { path: log_file },
        ),
        Err(error) => (None, FileLoggingOutcome::FileUnavailable { path: log_file, error }),
    }
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        debug_remnants,
        reason = "these warnings report that the subscriber itself could not be installed, so there is no `tracing` event to record them as"
    )
)]
pub fn init_logger<E, F, A>(service_name: ServiceName, resource_attributes: A, env: &E, fs: &F)
where
    E: ReadEnv,
    F: CreateDirAll + OpenAppendFile,
    A: IntoIterator<Item = ResourceAttribute>,
{
    // The providers and the subscriber are process-wide. A second call would build exporters
    // nobody stops and a subscriber that cannot be installed, so only the first call runs.
    if LOGGER_INITIALIZED.set(()).is_err() {
        eprintln!("WARN: logger already initialized");
        return;
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE)
        .json();

    let (log_file, file_logging) = try_open_log_file(service_name, env, fs);
    let file_layer = log_file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .json()
    });

    match try_init_otel(service_name, resource_attributes, env) {
        Ok((tracer_provider, meter_provider, logger_provider)) => {
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

            let tracer = tracer_provider.tracer(service_name.as_str().to_owned());
            let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            // The guard above makes this the only call that reaches here, so the cells are empty.
            opentelemetry::global::set_tracer_provider(tracer_provider.clone());
            let _ = trace::TRACER_PROVIDER.set(tracer_provider);

            opentelemetry::global::set_meter_provider(meter_provider.clone());
            let _ = metric::METER_PROVIDER.set(meter_provider);

            let otel_logs_layer = OpenTelemetryTracingBridge::new(&logger_provider);
            let _ = log::LOGGER_PROVIDER.set(logger_provider);

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
            file_logging.record();
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
            file_logging.record();
        }
    }
}

fn try_init_otel<A>(
    service_name: ServiceName,
    resource_attributes: A,
    env: &impl ReadEnv,
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
    let resource = resource::build(service_name, resource_attributes, env);

    let tracer_provider = trace::init_provider(&resource)?;
    let meter_provider = metric::init_provider(&resource)?;
    let logger_provider = log::init_provider(&resource)?;

    Ok((tracer_provider, meter_provider, logger_provider))
}

/// Flushes and shuts down the OpenTelemetry providers, giving up after
/// [`constants::SHUTDOWN_TIMEOUT`] in total even when the collector is
/// unreachable. Telemetry that has not been exported by then is dropped.
pub fn shutdown_otel() -> Result<(), TelemetryShutdownError> {
    tracing::info!("Shutting down OpenTelemetry providers");
    shutdown_providers(
        [trace::shutdown, metric::shutdown, log::shutdown],
        constants::SHUTDOWN_TIMEOUT,
    )
}

type ProviderShutdown = fn(Duration) -> Result<(), TelemetryProviderShutdownError>;

fn shutdown_providers(providers: [ProviderShutdown; 3], timeout: Duration) -> Result<(), TelemetryShutdownError> {
    let (sender, receiver) = mpsc::sync_channel(providers.len());
    for (index, shutdown) in providers.into_iter().enumerate() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send((index, shutdown(timeout)));
        });
    }
    drop(sender);

    let deadline = Instant::now() + timeout;
    let mut outcomes: [Option<Result<(), TelemetryProviderShutdownError>>; 3] = [None, None, None];
    while outcomes.iter().any(Option::is_none) {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((index, outcome)) => outcomes[index] = Some(outcome),
            Err(_) => break,
        }
    }

    let errors: Vec<_> = outcomes
        .into_iter()
        .enumerate()
        .filter_map(|(index, outcome)| match outcome {
            Some(Ok(())) => None,
            Some(Err(error)) => Some(error),
            None => Some(TelemetryProviderShutdownError::timed_out(index, timeout)),
        })
        .collect();
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
