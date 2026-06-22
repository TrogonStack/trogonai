use buffa::Message as _;
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};

use super::{LightEventCase, v1};
use crate::codec::{decode_event_case, event_type};

pub type LightEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1::LightEvent {
    type Error = LightEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        self.event
            .as_ref()
            .map(encode_light_event_case)
            .ok_or(LightEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1::LightEvent {
    type Error = LightEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match decode_light_event_case(event)? {
            Some(event) => Ok(EventDecodeOutcome::Decoded(v1::LightEvent { event: Some(event) })),
            None => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl EventType for v1::LightEvent {
    type Error = LightEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        self.event
            .as_ref()
            .map(light_event_case_type)
            .ok_or(LightEventPayloadError::MissingEvent)
    }
}

fn encode_light_event_case(event: &LightEventCase) -> Vec<u8> {
    match event {
        LightEventCase::LightTurnedOn(inner) => inner.encode_to_vec(),
    }
}

fn decode_light_event_case(event: EventData<'_>) -> Result<Option<LightEventCase>, LightEventPayloadError> {
    let Some(event) = decode_event_case::<v1::LightTurnedOn, LightEventCase>(&event) else {
        return Ok(None);
    };

    event.map(Some).map_err(LightEventPayloadError::Decode)
}

fn light_event_case_type(event: &LightEventCase) -> &'static str {
    match event {
        LightEventCase::LightTurnedOn(_) => event_type::<v1::LightTurnedOn>(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TurnOnDecideError {
    #[error("light '{light_id}' is already on")]
    AlreadyOn { light_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("light event is missing its event case")]
pub struct TurnOnEvolveError;

/// Domain command decoded from a [`v1::TurnOn`] proto wire message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOnCommand {
    pub light_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("turn-on command is missing light_id")]
pub struct TurnOnCommandDecodeError;

impl TryFrom<v1::TurnOn> for TurnOnCommand {
    type Error = TurnOnCommandDecodeError;

    fn try_from(value: v1::TurnOn) -> Result<Self, Self::Error> {
        if value.light_id.is_empty() {
            return Err(TurnOnCommandDecodeError);
        }
        Ok(Self {
            light_id: value.light_id,
        })
    }
}

impl From<TurnOnCommand> for v1::TurnOn {
    fn from(value: TurnOnCommand) -> Self {
        Self {
            light_id: value.light_id,
        }
    }
}

#[cfg(test)]
mod tests;
