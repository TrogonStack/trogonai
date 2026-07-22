//! Env-parsing entry point for the a2a-nats-stdio binary.
//!
//! Same coverage-friendly split as `a2a-nats-server`: env validation is pure
//! and testable; the NATS connect-and-pump half lives in `main.rs` behind
//! `cfg(not(coverage))` because `trogon-nats::NatsJetStreamClient` is excluded
//! during coverage builds.

use a2a_nats::{A2aAgentId, A2aPrefix, A2aPrefixError, AgentIdError, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX};
use trogon_nats::connect::ConnectError;
use trogon_std::env::ReadEnv;

#[cfg(not(coverage))]
use a2a_nats::client::A2aClient;
#[cfg(not(coverage))]
use a2a_nats::{Config, apply_timeout_overrides, nats_connect_timeout};
#[cfg(not(coverage))]
use trogon_nats::jetstream::NatsJetStreamClient;
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::signal::shutdown_signal;

#[cfg(not(coverage))]
use crate::io_loop::run_io_loop;

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
mod tests;
