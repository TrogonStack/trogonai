//! Its own test binary: only the first subscriber installed in a process exports spans.

use trogon_sandbox_observability::{OTLP_ENDPOINT_ENV, init_tracing_with_env};
use trogon_std::env::InMemoryEnv;

#[test]
fn a_blank_explicit_endpoint_falls_back_to_the_environment() -> Result<(), Box<dyn std::error::Error>> {
    let env = InMemoryEnv::new();
    env.set(OTLP_ENDPOINT_ENV, "http://127.0.0.1:9");
    assert!(init_tracing_with_env("worker", Some("  "), &env)?.exports_spans());
    Ok(())
}
