use trogon_sandbox_observability::{ObservabilityError, init_tracing_with_env};
use trogon_std::env::InMemoryEnv;

#[test]
fn an_invalid_rust_log_is_refused_not_replaced_by_info() {
    let env = InMemoryEnv::new();
    env.set("RUST_LOG", "info,[unclosed");
    let result = init_tracing_with_env("worker", None, &env);
    assert!(matches!(result, Err(ObservabilityError::LogFilter(_))));
}

#[test]
fn an_unset_rust_log_defaults_to_info() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing_with_env("worker", None, &InMemoryEnv::new())?;
    Ok(())
}
