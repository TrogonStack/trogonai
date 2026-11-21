#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! The OpenTelemetry providers and the tracing subscriber are process-wide, so the repeated
//! calls run in a child process: the parent can then read the child's stderr to check that
//! the second `init_logger` call did nothing beyond warning once.

use std::process::Command;

use trogon_std::env::{InMemoryEnv, ReadEnv, SystemEnv};
use trogon_std::fs::MemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName, TelemetryProviderShutdownError};

const CHILD_MARKER: &str = "TROGON_TELEMETRY_REPEATED_LIFECYCLE_CHILD";

#[test]
fn a_repeated_init_is_a_no_op_and_a_second_shutdown_reports_every_provider() {
    if SystemEnv.var(CHILD_MARKER).is_ok() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/tmp/test-repeated-lifecycle");
        let fs = MemFs::new();

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
            "the first shutdown stops the providers installed by the first init"
        );
        let error = trogon_telemetry::shutdown_otel()
            .expect_err("stopped providers cannot be shut down again");
        let failed = error.errors();
        assert_eq!(failed.len(), 3, "every provider must report a failure: {error}");
        assert!(matches!(failed[0], TelemetryProviderShutdownError::Tracer { .. }), "{error}");
        assert!(matches!(failed[1], TelemetryProviderShutdownError::Meter { .. }), "{error}");
        assert!(matches!(failed[2], TelemetryProviderShutdownError::Logger { .. }), "{error}");
        return;
    }

    // libtest names the test thread after the test, so the child runs exactly this one.
    // `--nocapture` lets the child's `eprintln!` warnings reach its stderr.
    let test_name = std::thread::current().name().unwrap().to_owned();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test_name, "--test-threads=1", "--nocapture"])
        .env(CHILD_MARKER, "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the child's lifecycle checks must pass: {stderr}"
    );
    assert_eq!(
        stderr.matches("WARN: logger already initialized").count(),
        1,
        "the second init must stop at the guard and warn once: {stderr}"
    );
    assert!(
        !stderr.contains("tracing subscriber already initialized"),
        "the second init must not try to install another subscriber: {stderr}"
    );
}
