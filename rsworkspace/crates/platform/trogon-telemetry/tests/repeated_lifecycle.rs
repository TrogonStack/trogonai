#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! The OpenTelemetry providers and the tracing subscriber are process-wide, so the repeated
//! calls run in a child process: the parent can then read the child's stderr to check that
//! the second `init_logger` call did nothing beyond warning once.

mod support;

use opentelemetry_otlp::{
    OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_LOGS_ENDPOINT, OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
};
use trogon_std::fs::MemFs;
use trogon_telemetry::TelemetryProviderShutdownError::{Logger, Meter, Tracer};

const CHILD_MARKER: &str = "TROGON_TELEMETRY_REPEATED_LIFECYCLE_CHILD";

#[test]
fn a_repeated_init_is_a_no_op_and_a_second_shutdown_reports_every_provider() {
    let Some(output) = support::rerun_as_child(CHILD_MARKER, |command| {
        // The lifecycle checks need OpenTelemetry setup to succeed, so the exporters get a
        // valid endpoint and no signal-specific override may leak in from the caller.
        command
            .env(OTEL_EXPORTER_OTLP_ENDPOINT, "http://127.0.0.1:4318")
            .env_remove(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT)
            .env_remove(OTEL_EXPORTER_OTLP_METRICS_ENDPOINT)
            .env_remove(OTEL_EXPORTER_OTLP_LOGS_ENDPOINT)
    }) else {
        let fs = MemFs::new();
        for _ in 0..2 {
            support::init_test_logger("/tmp/test-repeated-lifecycle", &fs);
        }

        assert!(
            trogon_telemetry::shutdown_otel().is_ok(),
            "the first shutdown stops the providers installed by the first init"
        );
        let error = trogon_telemetry::shutdown_otel().expect_err("stopped providers cannot be shut down again");
        assert!(
            matches!(error.errors(), [Tracer { .. }, Meter { .. }, Logger { .. }]),
            "every provider must report a failure, in order: {error}"
        );
        return;
    };
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
