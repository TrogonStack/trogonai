use super::*;
use trogon_std::env::InMemoryEnv;

#[test]
fn config_defaults_to_mcp_prefix() {
    let env = InMemoryEnv::new();
    let config = Config::from_env(&env).unwrap();
    assert_eq!(config.prefix_str(), "mcp");
    assert_eq!(config.nats().servers, vec!["localhost:4222"]);
}

#[test]
fn config_reads_prefix_and_nats_from_env() {
    let env = InMemoryEnv::new();
    env.set("MCP_PREFIX", "tenant.mcp");
    env.set("NATS_URL", "nats:4222");
    let config = Config::from_env(&env).unwrap();
    assert_eq!(config.prefix_str(), "tenant.mcp");
    assert_eq!(config.nats().servers, vec!["nats:4222"]);
}

#[test]
fn operation_timeout_override_respects_minimum() {
    let env = InMemoryEnv::new();
    env.set("MCP_OPERATION_TIMEOUT_SECS", "60");
    let config = apply_timeout_overrides(Config::from_env(&env).unwrap(), &env);
    assert_eq!(config.operation_timeout(), Duration::from_secs(60));

    env.set("MCP_OPERATION_TIMEOUT_SECS", "0");
    let config = apply_timeout_overrides(Config::from_env(&env).unwrap(), &env);
    assert_eq!(config.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
}

#[test]
fn operation_timeout_builder_respects_minimum() {
    let env = InMemoryEnv::new();
    let config = Config::from_env(&env).unwrap().with_operation_timeout(Duration::ZERO);
    assert_eq!(config.operation_timeout(), minimum_operation_timeout());

    let config = Config::from_env(&env)
        .unwrap()
        .with_operation_timeout(Duration::from_millis(1));
    assert_eq!(config.operation_timeout(), minimum_operation_timeout());
}

#[test]
fn operation_timeout_override_ignores_invalid_integer() {
    let env = InMemoryEnv::new();
    env.set("MCP_OPERATION_TIMEOUT_SECS", "slow");

    let config = apply_timeout_overrides(Config::from_env(&env).unwrap(), &env);

    assert_eq!(config.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
}

#[test]
fn connect_timeout_override_respects_minimum() {
    let env = InMemoryEnv::new();
    assert_eq!(nats_connect_timeout(&env), Duration::from_secs(10));
    env.set("MCP_NATS_CONNECT_TIMEOUT_SECS", "15");
    assert_eq!(nats_connect_timeout(&env), Duration::from_secs(15));
    env.set("MCP_NATS_CONNECT_TIMEOUT_SECS", "0");
    assert_eq!(nats_connect_timeout(&env), Duration::from_secs(10));
}

#[test]
fn connect_timeout_override_ignores_invalid_integer() {
    let env = InMemoryEnv::new();
    env.set("MCP_NATS_CONNECT_TIMEOUT_SECS", "slow");

    assert_eq!(nats_connect_timeout(&env), Duration::from_secs(10));
}
