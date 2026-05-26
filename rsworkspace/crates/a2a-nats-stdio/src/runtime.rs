use std::fmt;

use a2a_nats::client::Client;
use a2a_nats::{
    A2aAgentId, A2aPrefixError, AgentIdError, Config, ENV_A2A_PREFIX, apply_timeout_overrides, nats_connect_timeout,
};
use trogon_nats::connect::ConnectError;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::env::SystemEnv;
use trogon_std::signal::shutdown_signal;

use crate::io_loop::run_io_loop;

#[derive(Debug)]
pub enum RuntimeError {
    MissingAgentId,
    InvalidPrefix(A2aPrefixError),
    InvalidAgentId(AgentIdError),
    NatsConnect(ConnectError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAgentId => write!(f, "A2A_AGENT_ID environment variable is required"),
            Self::InvalidPrefix(e) => write!(f, "invalid A2A prefix: {e}"),
            Self::InvalidAgentId(e) => write!(f, "invalid agent id: {e}"),
            Self::NatsConnect(e) => write!(f, "NATS connection failed: {e}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPrefix(e) => Some(e),
            Self::InvalidAgentId(e) => Some(e),
            Self::NatsConnect(e) => Some(e),
            _ => None,
        }
    }
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

pub async fn run() -> Result<(), RuntimeError> {
    let env = SystemEnv;

    let raw_prefix = trogon_std::env::ReadEnv::var(&env, ENV_A2A_PREFIX)
        .unwrap_or_else(|_| a2a_nats::DEFAULT_A2A_PREFIX.to_string());
    let prefix = a2a_nats::A2aPrefix::new(raw_prefix)?;

    let agent_id_str = trogon_std::env::ReadEnv::var(&env, "A2A_AGENT_ID").map_err(|_| RuntimeError::MissingAgentId)?;
    let agent_id = A2aAgentId::new(agent_id_str)?;

    let nats_config = trogon_nats::NatsConfig::from_env(&env);
    let connect_timeout = nats_connect_timeout(&env);

    let nats_client = trogon_nats::connect(&nats_config, connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = NatsJetStreamClient::new(js_context);

    let config = Config::new(prefix, nats_config);
    let config = apply_timeout_overrides(config, &env);

    let client = Client::new(config, agent_id, nats_client, js_client);

    run_io_loop(client, tokio::io::stdin(), tokio::io::stdout(), shutdown_signal()).await;

    Ok(())
}
