//! Env-parsing entry point for the a2a-nats-stdio binary.
//!
//! Same coverage-friendly split as `a2a-nats-server`: env validation is pure
//! and testable; the NATS connect-and-pump half lives in `main.rs` behind
//! `cfg(not(coverage))` because `trogon-nats::NatsJetStreamClient` is excluded
//! during coverage builds.

use a2a_nats::{A2aAgentId, A2aPrefix, A2aPrefixError, AgentIdError, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX};
use trogon_nats::connect::ConnectError;
use trogon_std::env::ReadEnv;

pub(crate) const ENV_A2A_AGENT_ID: &str = "A2A_AGENT_ID";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("A2A_AGENT_ID environment variable is required")]
    MissingAgentId,
    #[error("invalid A2A prefix")]
    InvalidPrefix(#[source] A2aPrefixError),
    #[error("invalid agent id")]
    InvalidAgentId(#[source] AgentIdError),
    #[error("NATS connection failed")]
    NatsConnect(#[source] ConnectError),
    #[error("stdio loop failed")]
    IoLoop(#[source] std::io::Error),
}

impl From<A2aPrefixError> for RuntimeError {
    fn from(e: A2aPrefixError) -> Self {
        Self::InvalidPrefix(e)
    }
}

impl From<AgentIdError> for RuntimeError {
    fn from(e: AgentIdError) -> Self {
        Self::InvalidAgentId(e)
    }
}

impl From<ConnectError> for RuntimeError {
    fn from(e: ConnectError) -> Self {
        Self::NatsConnect(e)
    }
}

pub struct ValidatedStdioConfig {
    pub prefix: A2aPrefix,
    pub agent_id: A2aAgentId,
}

pub fn parse_env<E: ReadEnv>(env: &E) -> Result<ValidatedStdioConfig, RuntimeError> {
    let raw_prefix = env
        .var(ENV_A2A_PREFIX)
        .unwrap_or_else(|_| DEFAULT_A2A_PREFIX.to_string());
    let prefix = A2aPrefix::new(raw_prefix)?;
    let agent_id_str = env.var(ENV_A2A_AGENT_ID).map_err(|_| RuntimeError::MissingAgentId)?;
    let agent_id = A2aAgentId::new(agent_id_str)?;
    Ok(ValidatedStdioConfig { prefix, agent_id })
}

/// Backwards-compatible entry point used by integration tests / external
/// callers that want to drive the binary through the library. Returns
/// `RuntimeError` on env validation failure; connect-and-pump is in main.rs.
#[cfg(not(coverage))]
pub async fn run() -> Result<(), RuntimeError> {
    use a2a_nats::client::A2aClient;
    use a2a_nats::{Config, apply_timeout_overrides, nats_connect_timeout};
    use trogon_nats::jetstream::NatsJetStreamClient;
    use trogon_std::env::SystemEnv;
    use trogon_std::signal::shutdown_signal;

    use crate::io_loop::run_io_loop;

    let env = SystemEnv;
    let validated = parse_env(&env)?;

    let nats_config = trogon_nats::NatsConfig::from_env(&env);
    let connect_timeout = nats_connect_timeout(&env);
    let nats_client = trogon_nats::connect(&nats_config, connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = NatsJetStreamClient::new(js_context);

    // Honour A2A_OPERATION_TIMEOUT_SECS for unary + streaming calls — the env
    // var was being read into `Config` but never applied to the A2aClient
    // builder, so operators saw the env knob silently no-op.
    let config = apply_timeout_overrides(Config::new(validated.prefix.clone(), nats_config), &env);

    let client = A2aClient::new(validated.prefix, validated.agent_id, nats_client, js_client)
        .with_operation_timeout(config.operation_timeout());

    run_io_loop(client, tokio::io::stdin(), tokio::io::stdout(), shutdown_signal())
        .await
        .map_err(RuntimeError::IoLoop)
}

#[cfg(coverage)]
pub async fn run() -> Result<(), RuntimeError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use trogon_std::env::InMemoryEnv;

    use super::*;

    #[test]
    fn runtime_error_display_missing_agent_id() {
        assert_eq!(
            RuntimeError::MissingAgentId.to_string(),
            "A2A_AGENT_ID environment variable is required"
        );
    }

    #[test]
    fn runtime_error_display_invalid_prefix() {
        let e = RuntimeError::InvalidPrefix(A2aPrefix::new("").unwrap_err());
        assert_eq!(e.to_string(), "invalid A2A prefix");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_invalid_agent_id() {
        let e = RuntimeError::InvalidAgentId(A2aAgentId::new("a.b").unwrap_err());
        assert_eq!(e.to_string(), "invalid agent id");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn runtime_error_display_and_source_for_nats_connect() {
        let inner = ConnectError::InvalidCredentials(std::io::Error::other("oops"));
        let e = RuntimeError::NatsConnect(inner);
        assert_eq!(e.to_string(), "NATS connection failed");
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
        env.set(ENV_A2A_PREFIX, "bad prefix!");
        env.set(ENV_A2A_AGENT_ID, "bot");
        let result = parse_env(&env);
        assert!(matches!(result, Err(RuntimeError::InvalidPrefix(_))));
    }

    #[test]
    fn parse_env_valid_uses_default_prefix() {
        let env = InMemoryEnv::new();
        env.set(ENV_A2A_AGENT_ID, "bot");
        let cfg = parse_env(&env).expect("valid");
        assert_eq!(cfg.prefix.as_str(), DEFAULT_A2A_PREFIX);
        assert_eq!(cfg.agent_id.as_str(), "bot");
    }

    #[tokio::test]
    #[cfg(coverage)]
    async fn coverage_run_stub_is_callable() {
        super::run().await.unwrap();
    }

    #[test]
    fn runtime_error_display_and_source_for_io_loop() {
        let e = RuntimeError::IoLoop(std::io::Error::other("nope"));
        assert_eq!(e.to_string(), "stdio loop failed");
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn from_impls_construct_variants() {
        let e: RuntimeError = A2aPrefix::new("").unwrap_err().into();
        assert!(matches!(e, RuntimeError::InvalidPrefix(_)));
        let e: RuntimeError = A2aAgentId::new("a.b").unwrap_err().into();
        assert!(matches!(e, RuntimeError::InvalidAgentId(_)));
        let e: RuntimeError = ConnectError::InvalidCredentials(std::io::Error::other("x")).into();
        assert!(matches!(e, RuntimeError::NatsConnect(_)));
    }
}
