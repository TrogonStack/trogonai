use trogonai_proto::agents::agents::{AgentEventCase, v1};

use super::domain::AgentId;

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum AgentEventFoldError {
    #[error("agent event has no payload")]
    MissingEvent,
    #[error("event for agent '{actual}' was found in agent '{expected}' state")]
    AgentIdMismatch { expected: String, actual: AgentId },
    #[error("AgentProvisioned occurred more than once")]
    DuplicateProvision,
    #[error("invalid event payload: {0}")]
    InvalidPayload(#[from] super::CommandWireError),
}

pub fn provisioned_payload(event: &v1::AgentEvent) -> Result<&v1::AgentProvisioned, AgentEventFoldError> {
    let AgentEventCase::AgentProvisioned(provisioned) =
        event.event.as_ref().ok_or(AgentEventFoldError::MissingEvent)?;
    Ok(provisioned)
}

pub fn ensure_command_identity(expected: &AgentId, actual: &AgentId) -> Result<(), AgentEventFoldError> {
    if expected != actual {
        return Err(AgentEventFoldError::AgentIdMismatch {
            expected: expected.as_str().to_string(),
            actual: actual.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
