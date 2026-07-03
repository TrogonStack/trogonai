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

/// Covers the `Err(e)` arm in `try_open_log_file` when `open_append` fails.
#[test]
fn try_open_log_file_reports_failed_to_create_when_open_append_fails() {
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

fn otel_endpoint_configured(env: &impl ReadEnv) -> bool {
    env.var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok()
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
