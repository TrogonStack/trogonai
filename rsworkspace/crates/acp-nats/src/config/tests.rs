use std::time::Duration;

use super::*;

fn default_nats() -> NatsConfig {
    NatsConfig {
        servers: vec!["localhost:4222".to_string()],
        auth: trogon_nats::NatsAuth::None,
    }
}

#[test]
fn config_new_accepts_validated_prefix() {
    let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
    assert_eq!(config.acp_prefix(), "acp");
}

#[test]
fn config_with_operation_timeout() {
    let config =
        Config::new(AcpPrefix::new("acp").unwrap(), default_nats()).with_operation_timeout(Duration::from_secs(60));
    assert_eq!(config.operation_timeout(), Duration::from_secs(60));
}

#[test]
fn config_with_prompt_timeout() {
    let config =
        Config::new(AcpPrefix::new("acp").unwrap(), default_nats()).with_prompt_timeout(Duration::from_secs(3600));
    assert_eq!(config.prompt_timeout(), Duration::from_secs(3600));
}

#[test]
fn config_with_max_concurrent_client_tasks() {
    let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats()).with_max_concurrent_client_tasks(32);
    assert_eq!(config.max_concurrent_client_tasks(), 32);
}

#[test]
fn config_default_max_concurrent_client_tasks_is_256() {
    let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
    assert_eq!(config.max_concurrent_client_tasks(), 256);
}

#[test]
fn config_with_max_concurrent_client_tasks_enforces_minimum() {
    let config = Config::for_test("acp").with_max_concurrent_client_tasks(0);
    assert_eq!(config.max_concurrent_client_tasks(), 1);
}

#[test]
fn config_nats_returns_nats_config() {
    let config = Config::new(AcpPrefix::new("acp").unwrap(), default_nats());
    assert_eq!(config.nats().servers.len(), 1);
    assert_eq!(config.nats().servers[0], "localhost:4222");
}

#[test]
fn config_with_prefix_accepts_validated_prefix() {
    let config = Config::with_prefix(AcpPrefix::new("acp").unwrap(), default_nats());
    assert_eq!(config.acp_prefix(), "acp");
}

fn with_subscriber<F: FnOnce()>(f: F) {
    use tracing_subscriber::util::SubscriberInitExt;
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();
    f();
}

#[test]
fn test_nats_connect_timeout_from_env() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        assert_eq!(
            nats_connect_timeout(&env),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
        );

        env.set(ENV_CONNECT_TIMEOUT_SECS, "15");
        assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
        env.set(ENV_CONNECT_TIMEOUT_SECS, "0");
        assert_eq!(
            nats_connect_timeout(&env),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
        );
        env.set(ENV_CONNECT_TIMEOUT_SECS, "not-a-number");
        assert_eq!(
            nats_connect_timeout(&env),
            Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS)
        );
    });
}

#[test]
fn test_operation_timeout_invalid_env_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default = Config::for_test("acp");
        let default_timeout = default.operation_timeout();

        env.set("ACP_OPERATION_TIMEOUT_SECS", "not-a-number");
        let invalid = apply_timeout_overrides(Config::for_test("acp"), &env);
        assert_eq!(invalid.operation_timeout(), default_timeout);
    });
}

#[test]
fn test_operation_timeout_under_min_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default = Config::for_test("acp");
        let default_timeout = default.operation_timeout();

        env.set("ACP_OPERATION_TIMEOUT_SECS", "0");
        let ignored = apply_timeout_overrides(Config::for_test("acp"), &env);
        assert_eq!(ignored.operation_timeout(), default_timeout);
    });
}

#[test]
fn test_operation_timeout_valid_override() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set("ACP_OPERATION_TIMEOUT_SECS", "60");
        let config = apply_timeout_overrides(Config::for_test("acp"), &env);
        assert_eq!(config.operation_timeout(), Duration::from_secs(60));
    });
}

#[test]
fn test_prompt_timeout_invalid_env_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default = Config::for_test("acp");
        let default_timeout = default.prompt_timeout();

        env.set("ACP_PROMPT_TIMEOUT_SECS", "not-a-number");
        let invalid = apply_timeout_overrides(Config::for_test("acp"), &env);
        assert_eq!(invalid.prompt_timeout(), default_timeout);
    });
}

#[test]
fn test_prompt_timeout_under_min_is_ignored() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        let default = Config::for_test("acp");
        let default_timeout = default.prompt_timeout();

        env.set("ACP_PROMPT_TIMEOUT_SECS", "0");
        let ignored = apply_timeout_overrides(Config::for_test("acp"), &env);
        assert_eq!(ignored.prompt_timeout(), default_timeout);
    });
}

#[test]
fn test_prompt_timeout_valid_override() {
    with_subscriber(|| {
        let env = trogon_std::env::InMemoryEnv::new();
        env.set("ACP_PROMPT_TIMEOUT_SECS", "3600");
        let config = apply_timeout_overrides(Config::for_test("acp"), &env);
        assert_eq!(config.prompt_timeout(), Duration::from_secs(3600));
    });
}
