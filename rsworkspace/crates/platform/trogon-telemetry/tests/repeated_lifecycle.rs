#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! The OpenTelemetry providers and the tracing subscriber are process-wide, so these
//! repeated calls live in their own test binary, where they cannot leak into other tests.

use trogon_std::env::InMemoryEnv;
use trogon_std::fs::MemFs;
use trogon_telemetry::{ResourceAttribute, ServiceName};

#[test]
fn a_second_shutdown_reports_every_provider_after_a_repeated_init() {
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
    let message = trogon_telemetry::shutdown_otel()
        .expect_err("stopped providers cannot be shut down again")
        .to_string();
    for provider in ["tracer", "meter", "logger"] {
        assert!(
            message.contains(&format!("failed to shutdown {provider} provider")),
            "the error must name the {provider} provider: {message}"
        );
    }
}
