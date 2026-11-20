use agent_client_protocol::{AcpAgent, AcpAgentConfig};
use std::str::FromStr;

/// Why a launch spec could not describe an agent to spawn.
#[derive(Debug, thiserror::Error)]
pub enum AgentLaunchError {
    #[error("the agent launch spec is empty")]
    Empty,
    #[error("the agent launch spec is not a command line or an ACP agent JSON config")]
    Invalid(#[source] agent_client_protocol::Error),
}

/// How to start the ACP agent this host fronts.
///
/// Accepts what the SDK's [`AcpAgent`] parser accepts: a command line with
/// optional leading `NAME=value` environment assignments
/// (`npx -y @zed-industries/claude-agent-acp`, `trogonai-agent acp`), or the
/// JSON form `{"command": ..., "args": [...], "env": {...}}`, which is the shape
/// Zed's `agent_servers` setting uses. The spawned child also inherits this
/// process's environment, so provider credentials can stay in the host's own
/// environment instead of the spec.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentLaunch(AcpAgentConfig);

impl AgentLaunch {
    pub fn parse(raw: &str) -> Result<Self, AgentLaunchError> {
        if raw.trim().is_empty() {
            return Err(AgentLaunchError::Empty);
        }
        AcpAgent::from_str(raw)
            .map(|agent| Self(agent.into_config()))
            .map_err(AgentLaunchError::Invalid)
    }

    pub fn config(&self) -> &AcpAgentConfig {
        &self.0
    }

    /// A fresh launcher for one child process. Each call spawns a new child
    /// when its connection starts.
    pub fn to_agent(&self) -> AcpAgent {
        AcpAgent::new(self.0.clone())
    }
}

/// Prints the command, its arguments, and only the *names* of the environment
/// variables the spec sets: their values may be credentials.
impl std::fmt::Debug for AgentLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let config = self.config();
        f.debug_struct("AgentLaunch")
            .field("command", &config.command())
            .field("args", &config.arguments())
            .field("env", &config.environment().keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests;
