#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! The OTLP exporters read their endpoint from the process environment, which a test
//! cannot change safely in-process. The test re-runs itself as a child process with an
//! unusable endpoint, and the child does the logging setup.

use std::process::Command;

use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName};

const TEST_NAME: &str = "logging_starts_without_otel_when_the_exporters_cannot_be_built";
const CHILD_MARKER: &str = "TROGON_TELEMETRY_INIT_FAILURE_CHILD";

#[test]
fn logging_starts_without_otel_when_the_exporters_cannot_be_built() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/tmp/test-otel-init-failure");
        let fs = MemFs::new();
        // The second call finds the subscriber the first one installed.
        for _ in 0..2 {
            trogon_telemetry::init_logger(
                ServiceName::AcpNatsStdio,
                [ResourceAttribute::acp_prefix("test")],
                &env,
                &fs,
            );
        }
        assert!(
            trogon_telemetry::shutdown_otel().is_ok(),
            "no providers were installed, so shutdown has nothing to fail on"
        );
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_MARKER, "1")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "not a uri")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "the child must set up logging: {stderr}");
    assert!(
        stderr.contains("Logger initialized without OpenTelemetry"),
        "an unusable exporter endpoint must fall back to logging without OpenTelemetry: {stderr}"
    );
}
