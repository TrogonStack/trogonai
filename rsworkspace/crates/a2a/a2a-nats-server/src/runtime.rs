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
mod tests;
