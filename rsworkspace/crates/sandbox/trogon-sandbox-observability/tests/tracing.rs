use clap::Parser;
use trogon_sandbox_observability::{
    OTLP_ENDPOINT_ENV, ObservabilityArgs, bootstrap, init_tracing, init_tracing_with_env,
};
use trogon_std::env::InMemoryEnv;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const UNREACHABLE: &str = "http://127.0.0.1:9";

#[test]
fn no_endpoint_means_no_export() -> TestResult {
    let guard = init_tracing_with_env("worker", None, &InMemoryEnv::new())?;
    assert!(!guard.exports_spans());
    Ok(())
}

#[test]
fn blank_endpoint_means_no_export() -> TestResult {
    let env = InMemoryEnv::new();
    env.set(OTLP_ENDPOINT_ENV, "   ");
    assert!(!init_tracing_with_env("worker", Some(""), &env)?.exports_spans());
    assert!(!init_tracing_with_env("worker", None, &env)?.exports_spans());
    Ok(())
}

#[test]
fn explicit_endpoint_enables_export() -> TestResult {
    let guard = init_tracing_with_env("worker", Some(UNREACHABLE), &InMemoryEnv::new())?;
    assert!(guard.exports_spans());
    Ok(())
}

#[test]
fn environment_endpoint_enables_export() -> TestResult {
    let env = InMemoryEnv::new();
    env.set(OTLP_ENDPOINT_ENV, UNREACHABLE);
    assert!(init_tracing_with_env("worker", None, &env)?.exports_spans());
    Ok(())
}

#[test]
fn system_environment_variant_initializes() -> TestResult {
    let guard = init_tracing("worker", None)?;
    drop(guard);
    Ok(())
}

#[tokio::test]
async fn bootstrap_returns_guard_and_registry() -> TestResult {
    let args = ObservabilityArgs::try_parse_from(["bin", "--metrics-addr", "127.0.0.1:0"])?;
    let (guard, registry) = bootstrap("worker", 8090, &args)?;
    assert!(!registry.gather().is_empty());
    drop(guard);
    Ok(())
}
