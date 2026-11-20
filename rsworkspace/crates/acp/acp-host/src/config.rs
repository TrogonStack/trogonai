use crate::agent_launch::{AgentLaunch, AgentLaunchError};
use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use trogon_std::env::ReadEnv;

/// The agent to launch, as a command line or Zed `agent_servers` JSON.
pub const ENV_AGENT: &str = "ACP_HOST_AGENT";

/// Why the environment did not describe a host.
#[derive(Debug, thiserror::Error)]
pub enum HostConfigError {
    #[error("{ENV_AGENT} is not set; name the ACP agent to launch, e.g. `trogonai-agent acp`")]
    MissingAgent,
    #[error("{ENV_AGENT} does not describe an agent")]
    Agent(#[source] AgentLaunchError),
    #[error("{} is not a usable ACP prefix", acp_nats::ENV_ACP_PREFIX)]
    Prefix(#[source] AcpPrefixError),
}

pub struct HostConfig {
    pub acp: Config,
    pub agent: AgentLaunch,
}

impl HostConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Result<Self, HostConfigError> {
        let raw_agent = env.var(ENV_AGENT).map_err(|_| HostConfigError::MissingAgent)?;
        let agent = AgentLaunch::parse(&raw_agent).map_err(HostConfigError::Agent)?;

        let raw_prefix = env
            .var(acp_nats::ENV_ACP_PREFIX)
            .unwrap_or_else(|_| acp_nats::DEFAULT_ACP_PREFIX.to_string());
        let prefix = AcpPrefix::new(raw_prefix).map_err(HostConfigError::Prefix)?;

        Ok(Self {
            acp: Config::with_prefix(prefix, NatsConfig::from_env(env)),
            agent,
        })
    }
}

#[cfg(test)]
mod tests;
