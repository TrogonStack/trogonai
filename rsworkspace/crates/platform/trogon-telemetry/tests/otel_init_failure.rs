#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! The OTLP exporters read their endpoint from the process environment, which a test
//! cannot change safely in-process. The test re-runs itself as a child process with an
//! unusable endpoint, and the child does the logging setup.

use std::process::Command;

use opentelemetry_otlp::{
    OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_LOGS_ENDPOINT, OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
};
use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName};

const CHILD_MARKER: &str = "TROGON_TELEMETRY_INIT_FAILURE_CHILD";

#[test]
fn logging_starts_without_otel_when_the_exporters_cannot_be_built() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/tmp/test-otel-init-failure");
        trogon_telemetry::init_logger(
            ServiceName::AcpNatsStdio,
            [ResourceAttribute::acp_prefix("test")],
            &env,
            &MemFs::new(),
        );
        return;
    }

    // libtest names the test thread after the test, so the child runs exactly this one.
    let test_name = std::thread::current().name().unwrap().to_owned();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test_name, "--test-threads=1"])
        .env(CHILD_MARKER, "1")
        .env(OTEL_EXPORTER_OTLP_ENDPOINT, "not a uri")
        // Signal-specific endpoints override the generic one, and RUST_LOG could filter out
        // the warning this test looks for, so neither may leak in from the caller.
        .env_remove(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT)
        .env_remove(OTEL_EXPORTER_OTLP_METRICS_ENDPOINT)
        .env_remove(OTEL_EXPORTER_OTLP_LOGS_ENDPOINT)
        .env("RUST_LOG", "info")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "the child must set up logging: {stderr}");
    assert!(
        stderr.contains("Logger initialized without OpenTelemetry"),
        "an unusable exporter endpoint must fall back to logging without OpenTelemetry: {stderr}"
    );
}
