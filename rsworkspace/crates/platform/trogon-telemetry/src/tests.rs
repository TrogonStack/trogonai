use super::*;
use std::io;
use std::path::{Path, PathBuf};
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::{MemAppendWriter, MemFs};
use trogon_std::log_capture::{CapturedEvent, CapturedEvents, LevelFilter};

fn record_events(outcome: &FileLoggingOutcome) -> Vec<CapturedEvent> {
    let events = CapturedEvents::new();
    let guard = events.install(LevelFilter::INFO);

    outcome.record();
    drop(guard);

    events.events()
}

fn single_event(events: &[CapturedEvent]) -> &CapturedEvent {
    let [event] = events else {
        panic!("expected exactly one event, got {events:?}");
    };
    event
}

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
        errors: vec![
            TelemetryProviderShutdownError::Tracer {
                source: anyhow::anyhow!("trace failed"),
            },
            TelemetryProviderShutdownError::Meter {
                source: anyhow::anyhow!("metric failed"),
            },
        ],
    };

    assert_eq!(
        error.to_string(),
        "failed to shutdown OpenTelemetry providers:\n  - failed to shutdown tracer provider: trace failed\n  - failed to shutdown meter provider: metric failed\n"
    );
}

#[test]
fn try_open_log_file_succeeds_with_env_override() {
    let env = InMemoryEnv::new();
    env.set("TROGON_LOG_DIR", "/tmp/test-logs");
    let fs = MemFs::new();

    let (writer, outcome) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);
    assert!(writer.is_some());
    let FileLoggingOutcome::Enabled { path } = outcome else {
        panic!("expected file logging to be enabled");
    };
    assert_eq!(path, PathBuf::from("/tmp/test-logs/acp-nats-stdio.log"));
}

#[test]
fn try_open_log_file_falls_back_to_platform_dir() {
    let env = InMemoryEnv::new();
    let fs = MemFs::new();

    let (writer, outcome) = try_open_log_file(ServiceName::AcpNatsServer, &env, &fs);
    assert!(writer.is_some());
    assert!(matches!(outcome, FileLoggingOutcome::Enabled { .. }));
}

#[test]
fn try_open_log_file_reports_disabled_when_dir_fails() {
    let env = InMemoryEnv::new();
    let fs = MemFs::new();
    fs.insert("/tmp/test-logs", "file-blocking-dir");
    env.set("TROGON_LOG_DIR", "/tmp/test-logs/sub");

    let (writer, outcome) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);
    assert!(writer.is_none());
    assert!(matches!(outcome, FileLoggingOutcome::DirectoryUnavailable { .. }));
}

#[test]
fn try_open_log_file_reports_open_append_error() {
    let env = InMemoryEnv::new();
    env.set("TROGON_LOG_DIR", "/tmp/test-logs");
    let fs = OpenAppendErrorFs::new();

    let (writer, outcome) = try_open_log_file(ServiceName::AcpNatsStdio, &env, &fs);

    assert!(writer.is_none());
    let FileLoggingOutcome::FileUnavailable { path, error } = outcome else {
        panic!("expected the log file to be unopenable");
    };
    assert_eq!(path, PathBuf::from("/tmp/test-logs/acp-nats-stdio.log"));
    assert_eq!(error.kind(), io::ErrorKind::Other);
}

#[test]
fn record_reports_the_enabled_log_file_as_a_field() {
    let outcome = FileLoggingOutcome::Enabled {
        path: PathBuf::from("/tmp/test-logs/acp-nats-stdio.log"),
    };

    let events = record_events(&outcome);

    let event = single_event(&events);
    assert_eq!(event.message(), Some("file logging enabled"));
    assert_eq!(event.field("log_file"), Some("/tmp/test-logs/acp-nats-stdio.log"));
}

#[test]
fn record_reports_an_unavailable_directory_as_a_field() {
    let outcome = FileLoggingOutcome::DirectoryUnavailable {
        error: anyhow::anyhow!("permission denied"),
    };

    let events = record_events(&outcome);

    let event = single_event(&events);
    assert_eq!(
        event.message(),
        Some("file logging disabled: log directory unavailable")
    );
    assert_eq!(event.field("error"), Some("permission denied"));
}

#[test]
fn record_reports_an_unopenable_log_file_as_fields() {
    let outcome = FileLoggingOutcome::FileUnavailable {
        path: PathBuf::from("/tmp/test-logs/acp-nats-stdio.log"),
        error: io::Error::other("open append failed"),
    };

    let events = record_events(&outcome);

    let event = single_event(&events);
    assert_eq!(
        event.message(),
        Some("file logging disabled: log file could not be opened")
    );
    assert_eq!(event.field("log_file"), Some("/tmp/test-logs/acp-nats-stdio.log"));
    assert_eq!(event.field("error"), Some("open append failed"));
}

#[test]
fn service_name_reexported() {
    assert_eq!(ServiceName::AcpNatsStdio.as_str(), "acp-nats-stdio");
    assert_eq!(ServiceName::AcpNatsServer.as_str(), "acp-nats-server");
}

#[test]
// Exercises the meter factory with a throwaway instrument that has no
// semantic-convention counterpart, so it builds the instrument inline.
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(telemetry_metric_construction, telemetry_metric_name_literal)
)]
fn meter_returns_named_meter() {
    let m = meter("coverage-test");
    let counter = m.u64_counter("c").build();
    counter.add(1, &[]);
    assert!(!format!("{:?}", m).is_empty());
}

#[test]
fn shutdown_otel_succeeds_when_providers_not_initialized() {
    assert!(shutdown_otel().is_ok());
}

#[test]
fn telemetry_shutdown_error_includes_logger_failure() {
    let error = TelemetryShutdownError {
        errors: vec![TelemetryProviderShutdownError::Logger {
            source: anyhow::anyhow!("logger failed"),
        }],
    };
    assert!(matches!(
        error.errors.as_slice(),
        [TelemetryProviderShutdownError::Logger { .. }]
    ));
}

fn finishes_at_once(_: Duration) -> Result<(), TelemetryProviderShutdownError> {
    Ok(())
}

fn never_finishes(_: Duration) -> Result<(), TelemetryProviderShutdownError> {
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn fails_at_once(_: Duration) -> Result<(), TelemetryProviderShutdownError> {
    Err(TelemetryProviderShutdownError::Logger {
        source: anyhow::anyhow!("rejected"),
    })
}

#[test]
fn shutdown_providers_succeeds_when_every_provider_finishes() {
    assert!(shutdown_providers([finishes_at_once; 3], Duration::from_secs(1)).is_ok());
}

#[test]
fn shutdown_providers_gives_up_on_a_provider_that_ignores_its_timeout() {
    let started = Instant::now();
    let error = shutdown_providers(
        [finishes_at_once, never_finishes, finishes_at_once],
        Duration::from_millis(200),
    )
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(matches!(
        error.errors.as_slice(),
        [TelemetryProviderShutdownError::Meter { .. }]
    ));
}

#[test]
fn shutdown_providers_runs_providers_concurrently_and_reports_each_failure() {
    let started = Instant::now();
    let error = shutdown_providers(
        [never_finishes, never_finishes, fails_at_once],
        Duration::from_millis(300),
    )
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(matches!(
        error.errors.as_slice(),
        [
            TelemetryProviderShutdownError::Tracer { .. },
            TelemetryProviderShutdownError::Meter { .. },
            TelemetryProviderShutdownError::Logger { .. }
        ]
    ));
}

#[test]
fn the_shutdown_bound_is_three_seconds() {
    assert_eq!(constants::SHUTDOWN_TIMEOUT, Duration::from_secs(3));
}
