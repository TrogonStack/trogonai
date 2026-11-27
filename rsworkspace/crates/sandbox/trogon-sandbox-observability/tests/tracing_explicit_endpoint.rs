//! Its own test binary: only the first subscriber installed in a process exports spans.

use trogon_sandbox_observability::init_tracing_with_env;
use trogon_std::env::InMemoryEnv;

#[test]
fn explicit_endpoint_enables_export() -> Result<(), Box<dyn std::error::Error>> {
    let guard = init_tracing_with_env("worker", Some("http://127.0.0.1:9"), &InMemoryEnv::new())?;
    assert!(guard.exports_spans());
    Ok(())
}
