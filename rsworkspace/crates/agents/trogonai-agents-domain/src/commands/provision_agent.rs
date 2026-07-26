use buffa::MessageField;
use trogon_decider::{Decider, Decision};
use trogonai_proto::agents::agents::{state_v1, v1};

use super::CommandWireError;
use super::domain::{AgentDefinition, AgentId, ContentDigest, RevisionNumber};
use super::event_fold::{AgentEventFoldError, ensure_command_identity, provisioned_payload};
use super::proto_wire::{
    charter_to_wire, provisioned_agent_from_event, provisioned_agent_from_state, provisioned_agent_to_state,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionAgent {
    pub agent_id: AgentId,
    pub definition: AgentDefinition,
    pub content_digest: ContentDigest,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ProvisionAgentError {
    #[error("agent '{agent_id}' is already provisioned with the same definition")]
    AlreadyProvisionedIdentical { agent_id: AgentId },
    #[error("agent '{agent_id}' is already provisioned with a different definition")]
    AlreadyProvisionedConflict { agent_id: AgentId },
    #[error("provision state is invalid: {0}")]
    InvalidState(#[source] AgentEventFoldError),
    #[error("provision state payload is invalid: {0}")]
    InvalidStatePayload(#[source] CommandWireError),
}

impl Decider for ProvisionAgent {
    type StreamId = str;
    type State = state_v1::ProvisionAgentState;
    type Event = v1::AgentEvent;
    type DecideError = ProvisionAgentError;
    type EvolveError = AgentEventFoldError;

    fn stream_id(&self) -> &Self::StreamId {
        self.agent_id.as_str()
    }

    fn initial_state() -> Self::State {
        state_v1::ProvisionAgentState::default()
    }

    fn evolve(mut state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        let provisioned = provisioned_payload(event)?;
        if state.provisioned.as_option().is_some() {
            return Err(AgentEventFoldError::DuplicateProvision);
        }
        let identity = provisioned_agent_from_event(provisioned)?;
        state.provisioned = MessageField::some(provisioned_agent_to_state(&identity));
        Ok(state)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let Some(existing) = state.provisioned.as_option() else {
            let definition = &command.definition;
            return Ok(Decision::event(v1::AgentEvent {
                event: Some(
                    v1::AgentProvisioned {
                        agent_id: command.agent_id.as_str().to_string(),
                        name: definition.name().as_str().to_string(),
                        parent: definition.parent().as_str().to_string(),
                        charter: MessageField::some(charter_to_wire(definition.charter())),
                        revision: MessageField::some(v1::RevisionRef {
                            number: RevisionNumber::GENESIS.get(),
                            content_digest: command.content_digest.as_str().to_string(),
                        }),
                    }
                    .into(),
                ),
            }));
        };

        let provisioned = provisioned_agent_from_state(existing).map_err(ProvisionAgentError::InvalidStatePayload)?;
        ensure_command_identity(&provisioned.agent_id, &command.agent_id).map_err(ProvisionAgentError::InvalidState)?;
        if provisioned.definition == command.definition && provisioned.content_digest == command.content_digest {
            Err(ProvisionAgentError::AlreadyProvisionedIdentical {
                agent_id: command.agent_id.clone(),
            })
        } else {
            Err(ProvisionAgentError::AlreadyProvisionedConflict {
                agent_id: command.agent_id.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests;
