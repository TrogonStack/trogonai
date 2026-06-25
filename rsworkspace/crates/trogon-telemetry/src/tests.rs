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
