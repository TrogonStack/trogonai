use crate::constants::{ACTOR_ARGUMENT, AGENT_PORT_ARGUMENT};
use crate::{GuestActor, GuestActorId, GuestActorIdError, GuestAgentPort, GuestBootConfigError};

/// What a guest learns from its kernel command line: the actor it serves and the port its agent
/// listens on.
///
/// A missing actor argument is an error: the host passes either an actor id or the unbound
/// marker, and only the marker means the guest is unbound. A missing port argument selects [`GuestAgentPort::DEFAULT`]. The first occurrence of each
/// argument wins.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GuestBootConfig {
    actor: GuestActor,
    agent_port: GuestAgentPort,
}

impl GuestBootConfig {
    pub fn from_kernel_command_line(command_line: &str) -> Result<Self, GuestBootConfigError> {
        let actor = match command_line
            .split_whitespace()
            .find_map(|argument| argument.strip_prefix(ACTOR_ARGUMENT))
        {
            None => return Err(GuestActorIdError::Missing.into()),
            Some(GuestActorId::UNBOUND_MARKER) => GuestActor::Unbound,
            Some(actor_id) => GuestActor::Bound(GuestActorId::new(actor_id)?),
        };
        let agent_port = match command_line
            .split_whitespace()
            .find_map(|argument| argument.strip_prefix(AGENT_PORT_ARGUMENT))
        {
            Some(port) => port.parse()?,
            None => GuestAgentPort::DEFAULT,
        };
        Ok(Self { actor, agent_port })
    }

    pub fn actor(&self) -> &GuestActor {
        &self.actor
    }

    pub fn agent_port(&self) -> GuestAgentPort {
        self.agent_port
    }
}
