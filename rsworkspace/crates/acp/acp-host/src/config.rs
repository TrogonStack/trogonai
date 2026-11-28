use crate::agent_launch::{AgentLaunch, AgentLaunchError};
use crate::constants::{DEFAULT_CLIENT_RETRY_WINDOW, ENV_AGENT, ENV_CLIENT_RETRY_WINDOW_SECS};
use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use std::time::Duration;
use trogon_std::env::ReadEnv;

/// Why the environment did not describe a host.
#[derive(Debug, thiserror::Error)]
pub enum HostConfigError {
    #[error("{ENV_AGENT} is not set; name the ACP agent to launch, e.g. `trogonai-agent acp`")]
    MissingAgent,
    #[error("{ENV_AGENT} does not describe an agent")]
    Agent(#[source] AgentLaunchError),
    #[error("{} is not a usable ACP prefix", acp_nats::ENV_ACP_PREFIX)]
    Prefix(#[source] AcpPrefixError),
    #[error("{ENV_CLIENT_RETRY_WINDOW_SECS} is not a whole number of seconds: {0:?}")]
    ClientRetryWindow(String),
}

pub struct HostConfig {
    pub acp: Config,
    pub agent: AgentLaunch,
    /// How long a request the child sends to the client is sent again while
    /// nobody is subscribed (see [`crate::outage_tolerant_nats`]).
    pub client_retry_window: Duration,
}

impl HostConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Result<Self, HostConfigError> {
        let raw_agent = env.var(ENV_AGENT).map_err(|_| HostConfigError::MissingAgent)?;
        let agent = AgentLaunch::parse(&raw_agent).map_err(HostConfigError::Agent)?;

        let raw_prefix = env
            .var(acp_nats::ENV_ACP_PREFIX)
            .unwrap_or_else(|_| acp_nats::DEFAULT_ACP_PREFIX.to_string());
        let prefix = AcpPrefix::new(raw_prefix).map_err(HostConfigError::Prefix)?;

        let client_retry_window = match env.var(ENV_CLIENT_RETRY_WINDOW_SECS) {
            Ok(raw) => raw
                .trim()
                .parse()
                .map(Duration::from_secs)
                .map_err(|_| HostConfigError::ClientRetryWindow(raw))?,
            Err(_) => DEFAULT_CLIENT_RETRY_WINDOW,
        };

        Ok(Self {
            acp: Config::with_prefix(prefix, NatsConfig::from_env(env)),
            agent,
            client_retry_window,
        })
    }
}

#[cfg(test)]
mod tests;
