use super::*;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing_subscriber::util::SubscriberInitExt;
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::{MemAppendWriter, MemFs};

/// Collects what a subscriber writes so a test can assert which values reached
/// it as fields rather than as message text.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn record_to_string(outcome: &FileLoggingOutcome) -> String {
    let captured = CapturedLogs::default();
    let writer = captured.clone();
    let guard = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();

    outcome.record();
    drop(guard);

    let bytes = captured.0.lock().unwrap().clone();
    String::from_utf8(bytes).expect("subscriber output is utf-8")
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

    let logged = record_to_string(&outcome);

    assert!(logged.contains("file logging enabled"), "{logged}");
    assert!(
        logged.contains("log_file=/tmp/test-logs/acp-nats-stdio.log"),
        "{logged}"
    );
}

#[test]
fn record_reports_an_unavailable_directory_as_a_field() {
    let outcome = FileLoggingOutcome::DirectoryUnavailable {
        error: anyhow::anyhow!("permission denied"),
    };

    let logged = record_to_string(&outcome);

    assert!(
        logged.contains("file logging disabled: log directory unavailable"),
        "{logged}"
    );
    assert!(logged.contains("error=permission denied"), "{logged}");
}

#[test]
fn record_reports_an_unopenable_log_file_as_fields() {
    let outcome = FileLoggingOutcome::FileUnavailable {
        path: PathBuf::from("/tmp/test-logs/acp-nats-stdio.log"),
        error: io::Error::other("open append failed"),
    };

    let logged = record_to_string(&outcome);

    assert!(
        logged.contains("file logging disabled: log file could not be opened"),
        "{logged}"
    );
    assert!(
        logged.contains("log_file=/tmp/test-logs/acp-nats-stdio.log"),
        "{logged}"
    );
    assert!(logged.contains("error=open append failed"), "{logged}");
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
    dylint_lib = "trogon_lints",
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
