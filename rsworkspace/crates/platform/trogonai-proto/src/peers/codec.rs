use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};

use super::v1alpha1;

pub type PeerEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1alpha1::PeerEvent {
    type Error = PeerEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Err(PeerEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1alpha1::PeerEvent {
    type Error = PeerEventPayloadError;

    fn decode(_event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        Ok(EventDecodeOutcome::Skipped)
    }
}

impl EventType for v1alpha1::PeerEvent {
    type Error = PeerEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Err(PeerEventPayloadError::MissingEvent)
    }
}

#[cfg(test)]
mod tests;
