//! Env-parsing entry point for the a2a-nats-stdio binary.

use a2a_identity_types::{JwtError, MintedUserJwt, MintedUserJwtError};
use a2a_nats::constants::ENV_A2A_CALLER_JWT;
use a2a_nats::{A2aAgentId, A2aPrefix, A2aPrefixError, AgentIdError, DEFAULT_A2A_PREFIX, ENV_A2A_PREFIX};
use trogon_identity_types::SubjectPrincipal;
use trogon_nats::connect::ConnectError;
use trogon_std::env::ReadEnv;

use a2a_nats::client::A2aClient;
use a2a_nats::{Config, apply_timeout_overrides, nats_connect_timeout};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::env::SystemEnv;
use trogon_std::signal::shutdown_signal;

use crate::io_loop::run_io_loop;

use crate::constants::ENV_A2A_AGENT_ID;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("A2A_AGENT_ID environment variable is required")]
    MissingAgentId,
    #[error("invalid A2A prefix")]
    InvalidPrefix(#[source] A2aPrefixError),
    #[error("invalid agent id")]
    InvalidAgentId(#[source] AgentIdError),
    #[error("{} environment variable is required", ENV_A2A_CALLER_JWT)]
    MissingCallerJwt,
    #[error("invalid caller JWT")]
    InvalidCallerJwt(#[source] JwtError),
    #[error("caller JWT names no caller principal")]
    InvalidCallerPrincipal(#[source] MintedUserJwtError),
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
    pub caller: SubjectPrincipal,
}

pub fn parse_env<E: ReadEnv>(env: &E) -> Result<ValidatedStdioConfig, RuntimeError> {
    let raw_prefix = env
        .var(ENV_A2A_PREFIX)
        .unwrap_or_else(|_| DEFAULT_A2A_PREFIX.to_string());
    let prefix = A2aPrefix::new(raw_prefix)?;
    let agent_id_str = env.var(ENV_A2A_AGENT_ID).map_err(|_| RuntimeError::MissingAgentId)?;
    let agent_id = A2aAgentId::new(agent_id_str)?;
    let raw_jwt = env
        .var(ENV_A2A_CALLER_JWT)
        .map_err(|_| RuntimeError::MissingCallerJwt)?;
    let caller_jwt = MintedUserJwt::new(raw_jwt).map_err(RuntimeError::InvalidCallerJwt)?;
    caller_jwt.ensure_fresh().map_err(RuntimeError::InvalidCallerJwt)?;
    let caller = caller_jwt
        .subject_principal()
        .map_err(RuntimeError::InvalidCallerPrincipal)?;
    Ok(ValidatedStdioConfig {
        prefix,
        agent_id,
        caller,
    })
}

/// Backwards-compatible entry point used by integration tests / external
/// callers that want to drive the binary through the library.
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

    let client = A2aClient::new(
        validated.prefix,
        validated.agent_id,
        validated.caller,
        nats_client,
        js_client,
    )
    .with_operation_timeout(config.operation_timeout());

    run_io_loop(client, tokio::io::stdin(), tokio::io::stdout(), shutdown_signal())
        .await
        .map_err(RuntimeError::IoLoop)
}

#[cfg(test)]
mod tests;
