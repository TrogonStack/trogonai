//! Env-parsing entry point shared by main.rs.
//!
//! Splits the validation half (pure, testable, runs under coverage) from the
//! NATS-connect-and-serve half (gated to `cfg(not(coverage))` because the
//! upstream `trogon-nats` crate also gates `NatsJetStreamClient` out during
//! coverage builds).

use a2a_nats::{
    A2aAgentId, A2aPrefix, AgentIdError, Config, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX, NatsConfig,
    apply_timeout_overrides,
};
use trogon_std::env::ReadEnv;

pub const ENV_A2A_AGENT_ID: &str = "A2A_AGENT_ID";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("A2A_AGENT_ID env var is required but not set")]
    MissingAgentId,
    #[error("invalid agent id")]
    InvalidAgentId(#[source] AgentIdError),
    #[error("invalid A2A prefix")]
    InvalidPrefix(#[source] a2a_nats::A2aPrefixError),
    #[error("NATS connection failed")]
    NatsConnect(#[source] trogon_nats::ConnectError),
    #[error("JetStream provisioning failed")]
    Provision(#[source] a2a_nats::jetstream::ProvisionError),
    #[error("bridge error")]
    Bridge(#[source] a2a_nats::server::BridgeError),
}

/// Validated, ready-to-use configuration assembled from environment variables.
pub struct ValidatedRuntimeConfig {
    pub prefix: A2aPrefix,
    pub agent_id: A2aAgentId,
    pub nats_config: NatsConfig,
    pub config: Config,
}

/// Parse environment into a typed runtime configuration. Pure and testable
/// without a real NATS server.
pub fn parse_env<E: ReadEnv>(env: &E) -> Result<ValidatedRuntimeConfig, RuntimeError> {
    let prefix_raw = env
        .var(ENV_A2A_PREFIX)
        .unwrap_or_else(|_| DEFAULT_A2A_PREFIX.to_string());
    let prefix = A2aPrefix::new(prefix_raw).map_err(RuntimeError::InvalidPrefix)?;

    let agent_id_raw = env.var(ENV_A2A_AGENT_ID).map_err(|_| RuntimeError::MissingAgentId)?;
    let agent_id = A2aAgentId::new(&agent_id_raw).map_err(RuntimeError::InvalidAgentId)?;

    let nats_config = NatsConfig::from_env(env);
    let base_config = Config::new(prefix.clone(), nats_config.clone());
    let config = apply_timeout_overrides(base_config, env);

    Ok(ValidatedRuntimeConfig {
        prefix,
        agent_id,
        nats_config,
        config,
    })
}

#[cfg(test)]
mod tests {
    use trogon_std::env::InMemoryEnv;

    use super::*;

    #[test]
    fn runtime_error_display_missing_agent_id() {
        assert_eq!(
            RuntimeError::MissingAgentId.to_string(),
            "A2A_AGENT_ID env var is required but not set"
        );
    }

    #[test]
    fn runtime_error_display_and_source_for_invalid_agent_id() {
        let inner = A2aAgentId::new("a.b").unwrap_err();
        let e = RuntimeError::InvalidAgentId(inner);
        assert_eq!(e.to_string(), "invalid agent id");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_invalid_prefix() {
        let inner = A2aPrefix::new("").unwrap_err();
        let e = RuntimeError::InvalidPrefix(inner);
        assert_eq!(e.to_string(), "invalid A2A prefix");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_missing_agent_id_has_no_source() {
        let e = RuntimeError::MissingAgentId;
        assert!(std::error::Error::source(&e).is_none());
    }

    #[test]
    fn parse_env_missing_agent_id_returns_error() {
        let env = InMemoryEnv::new();
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::MissingAgentId)));
    }

    #[test]
    fn parse_env_invalid_agent_id_returns_error() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "a.b");
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::InvalidAgentId(_))));
    }

    #[test]
    fn parse_env_invalid_prefix_returns_error() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "bot");
        env.set(ENV_A2A_PREFIX, "bad prefix!");
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::InvalidPrefix(_))));
    }

    #[test]
    fn runtime_error_display_and_source_for_nats_connect() {
        let inner = trogon_nats::ConnectError::InvalidCredentials(std::io::Error::other("oops"));
        let e = RuntimeError::NatsConnect(inner);
        assert_eq!(e.to_string(), "NATS connection failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_provision() {
        let inner = a2a_nats::jetstream::ProvisionError("stream create failed".to_string());
        let e = RuntimeError::Provision(inner);
        assert_eq!(e.to_string(), "JetStream provisioning failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_bridge() {
        let inner = a2a_nats::server::BridgeError::Subscribe(Box::new(std::io::Error::other("denied")));
        let e = RuntimeError::Bridge(inner);
        assert_eq!(e.to_string(), "bridge error");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn parse_env_valid_sets_default_prefix() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "bot");
        let cfg = parse_env(&env).expect("valid env");
        assert_eq!(cfg.prefix.as_str(), DEFAULT_A2A_PREFIX);
        assert_eq!(cfg.agent_id.as_str(), "bot");
    }
}
