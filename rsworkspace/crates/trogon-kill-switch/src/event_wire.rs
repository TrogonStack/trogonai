use serde::{Deserialize, Serialize};
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

use crate::{AgentId, AgentIdError, KillReason, KillReasonError, KillSwitchEvent, OccurredAt, OccurredAtError};

const AGENT_KILLED_EVENT_TYPE: &str = "trogon.kill_switch.agent_killed.v1";
const AGENT_REVIVED_EVENT_TYPE: &str = "trogon.kill_switch.agent_revived.v1";

#[derive(Debug, thiserror::Error)]
pub enum KillSwitchEventCodecError {
    #[error("failed to serialize kill switch event payload: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize kill switch event payload: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("invalid agent id in event payload: {0}")]
    AgentId(#[source] AgentIdError),
    #[error("invalid kill reason in event payload: {0}")]
    KillReason(#[source] KillReasonError),
    #[error("invalid timestamp in event payload: {0}")]
    OccurredAt(#[source] OccurredAtError),
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentKilledPayload {
    agent_id: String,
    reason: String,
    occurred_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentRevivedPayload {
    agent_id: String,
    occurred_at: String,
}

impl EventType for KillSwitchEvent {
    type Error = std::convert::Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok(match self {
            Self::AgentKilled { .. } => AGENT_KILLED_EVENT_TYPE,
            Self::AgentRevived { .. } => AGENT_REVIVED_EVENT_TYPE,
        })
    }
}

impl EventEncode for KillSwitchEvent {
    type Error = KillSwitchEventCodecError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        match self {
            Self::AgentKilled {
                agent_id,
                reason,
                occurred_at,
            } => {
                let payload = AgentKilledPayload {
                    agent_id: agent_id.as_str().to_owned(),
                    reason: reason.as_str().to_owned(),
                    occurred_at: occurred_at
                        .to_rfc3339()
                        .map_err(KillSwitchEventCodecError::OccurredAt)?,
                };
                serde_json::to_vec(&payload).map_err(KillSwitchEventCodecError::Serialize)
            }
            Self::AgentRevived { agent_id, occurred_at } => {
                let payload = AgentRevivedPayload {
                    agent_id: agent_id.as_str().to_owned(),
                    occurred_at: occurred_at
                        .to_rfc3339()
                        .map_err(KillSwitchEventCodecError::OccurredAt)?,
                };
                serde_json::to_vec(&payload).map_err(KillSwitchEventCodecError::Serialize)
            }
        }
    }
}

impl EventDecode for KillSwitchEvent {
    type Error = KillSwitchEventCodecError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match event.event_type {
            AGENT_KILLED_EVENT_TYPE => {
                let payload: AgentKilledPayload =
                    serde_json::from_slice(event.payload).map_err(KillSwitchEventCodecError::Deserialize)?;
                let agent_id = AgentId::parse(&payload.agent_id).map_err(KillSwitchEventCodecError::AgentId)?;
                let reason = KillReason::parse(&payload.reason).map_err(KillSwitchEventCodecError::KillReason)?;
                let occurred_at =
                    OccurredAt::parse_rfc3339(&payload.occurred_at).map_err(KillSwitchEventCodecError::OccurredAt)?;
                Ok(EventDecodeOutcome::Decoded(Self::AgentKilled {
                    agent_id,
                    reason,
                    occurred_at,
                }))
            }
            AGENT_REVIVED_EVENT_TYPE => {
                let payload: AgentRevivedPayload =
                    serde_json::from_slice(event.payload).map_err(KillSwitchEventCodecError::Deserialize)?;
                let agent_id = AgentId::parse(&payload.agent_id).map_err(KillSwitchEventCodecError::AgentId)?;
                let occurred_at =
                    OccurredAt::parse_rfc3339(&payload.occurred_at).map_err(KillSwitchEventCodecError::OccurredAt)?;
                Ok(EventDecodeOutcome::Decoded(Self::AgentRevived {
                    agent_id,
                    occurred_at,
                }))
            }
            _ => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

#[cfg(test)]
mod tests;
