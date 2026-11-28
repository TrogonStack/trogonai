use super::*;
use std::path::Path;
use trogon_std::env::InMemoryEnv;

#[test]
fn the_agent_and_prefix_come_from_the_environment() {
    let env = InMemoryEnv::new();
    env.set(ENV_AGENT, "trogonai-agent acp");
    env.set(acp_nats::ENV_ACP_PREFIX, "team");

    let config = HostConfig::from_env(&env).expect("config");

    assert_eq!(config.agent.config().command(), Path::new("trogonai-agent"));
    assert_eq!(config.acp.acp_prefix(), "team");
}

#[test]
fn the_prefix_defaults_to_the_shared_acp_default() {
    let env = InMemoryEnv::new();
    env.set(ENV_AGENT, "gemini --acp");

    let config = HostConfig::from_env(&env).expect("config");

    assert_eq!(config.acp.acp_prefix(), acp_nats::DEFAULT_ACP_PREFIX);
}

#[test]
fn a_host_without_an_agent_refuses_to_start() {
    let env = InMemoryEnv::new();
    assert!(matches!(HostConfig::from_env(&env), Err(HostConfigError::MissingAgent)));
}

#[test]
fn an_unusable_prefix_is_reported_as_such() {
    let env = InMemoryEnv::new();
    env.set(ENV_AGENT, "gemini --acp");
    env.set(acp_nats::ENV_ACP_PREFIX, "has spaces");

    assert!(matches!(HostConfig::from_env(&env), Err(HostConfigError::Prefix(_))));
}

#[test]
fn the_client_retry_window_defaults_and_can_be_set_or_turned_off() {
    let env = InMemoryEnv::new();
    env.set(ENV_AGENT, "gemini --acp");
    let window = || HostConfig::from_env(&env).expect("config").client_retry_window;

    assert_eq!(window(), DEFAULT_CLIENT_RETRY_WINDOW);
    env.set(ENV_CLIENT_RETRY_WINDOW_SECS, "3");
    assert_eq!(window(), Duration::from_secs(3));
    env.set(ENV_CLIENT_RETRY_WINDOW_SECS, "0");
    assert_eq!(window(), Duration::ZERO);
}

#[test]
fn a_client_retry_window_that_is_not_whole_seconds_is_refused() {
    let env = InMemoryEnv::new();
    env.set(ENV_AGENT, "gemini --acp");
    env.set(ENV_CLIENT_RETRY_WINDOW_SECS, "1.5");

    assert!(matches!(
        HostConfig::from_env(&env),
        Err(HostConfigError::ClientRetryWindow(raw)) if raw == "1.5"
    ));
}
