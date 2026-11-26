use crate::{GuestAgentPort, GuestAgentPortError};

const ACTOR_ARGUMENT: &str = "ate.actor_id=";
const AGENT_PORT_ARGUMENT: &str = "sandbox.agent_port=";
const UNBOUND_ACTOR: &str = "unbound";

/// What a guest learns from its kernel command line: the actor it serves and the port its agent
/// listens on.
///
/// A missing actor argument means the guest is unbound; a missing port argument selects
/// [`GuestAgentPort::DEFAULT`]. The first occurrence of each argument wins.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GuestBootConfig {
    actor_id: String,
    agent_port: GuestAgentPort,
}

impl GuestBootConfig {
    pub fn from_kernel_command_line(command_line: &str) -> Result<Self, GuestAgentPortError> {
        let actor_id = command_line
            .split_whitespace()
            .find_map(|argument| argument.strip_prefix(ACTOR_ARGUMENT))
            .unwrap_or(UNBOUND_ACTOR)
            .to_owned();
        let agent_port = match command_line
            .split_whitespace()
            .find_map(|argument| argument.strip_prefix(AGENT_PORT_ARGUMENT))
        {
            Some(port) => port.parse()?,
            None => GuestAgentPort::DEFAULT,
        };
        Ok(Self { actor_id, agent_port })
    }

    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    pub fn agent_port(&self) -> GuestAgentPort {
        self.agent_port
    }
}
