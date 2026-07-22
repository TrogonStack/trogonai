use buffa::Message as _;
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};

use super::{AgentEventCase, v1};
use crate::codec::{decode_event_case, event_type};

#[cfg(feature = "runtime-host")]
use trogon_decider_runtime::EventIdentity;

pub type AgentEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1::AgentEvent {
    type Error = AgentEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        self.event
            .as_ref()
            .map(encode_agent_event_case)
            .ok_or(AgentEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1::AgentEvent {
    type Error = AgentEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match decode_agent_event_case(event)? {
            Some(event) => Ok(EventDecodeOutcome::Decoded(v1::AgentEvent { event: Some(event) })),
            None => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl EventType for v1::AgentEvent {
    type Error = AgentEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        self.event
            .as_ref()
            .map(agent_event_case_type)
            .ok_or(AgentEventPayloadError::MissingEvent)
    }
}

#[cfg(feature = "runtime-host")]
impl EventIdentity for v1::AgentEvent {}

fn encode_agent_event_case(event: &AgentEventCase) -> Vec<u8> {
    match event {
        AgentEventCase::AgentProvisioned(inner) => inner.encode_to_vec(),
    }
}

fn decode_agent_event_case(event: EventData<'_>) -> Result<Option<AgentEventCase>, AgentEventPayloadError> {
    let Some(event) = decode_event_case::<v1::AgentProvisioned, AgentEventCase>(&event) else {
        return Ok(None);
    };

    event.map(Some).map_err(AgentEventPayloadError::Decode)
}

fn agent_event_case_type(event: &AgentEventCase) -> &'static str {
    match event {
        AgentEventCase::AgentProvisioned(_) => event_type::<v1::AgentProvisioned>(),
    }
}

#[cfg(test)]
mod tests;
