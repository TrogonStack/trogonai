use buffa::Message as _;
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};

use super::{PeerEventCase, v1alpha1};
use crate::codec::{decode_event_case, event_type};

pub type PeerEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1alpha1::PeerEvent {
    type Error = PeerEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        self.event
            .as_ref()
            .map(encode_peer_event_case)
            .ok_or(PeerEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1alpha1::PeerEvent {
    type Error = PeerEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match decode_peer_event_case(event)? {
            Some(event) => Ok(EventDecodeOutcome::Decoded(v1alpha1::PeerEvent { event: Some(event) })),
            None => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl EventType for v1alpha1::PeerEvent {
    type Error = PeerEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        self.event
            .as_ref()
            .map(peer_event_case_type)
            .ok_or(PeerEventPayloadError::MissingEvent)
    }
}

#[cfg(feature = "peers-runtime")]
impl trogon_decider_runtime::EventIdentity for v1alpha1::PeerEvent {}

fn encode_peer_event_case(event: &PeerEventCase) -> Vec<u8> {
    match event {
        PeerEventCase::PeerEnrolled(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerAttached(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerHeartbeatRecorded(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerDetached(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerMessageAccepted(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerMessageIntakeRecorded(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerBatchPrepared(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerBatchAcknowledged(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerMessageDisposed(inner) => inner.encode_to_vec(),
        PeerEventCase::PeerMessageSettled(inner) => inner.encode_to_vec(),
    }
}

fn decode_peer_event_case(event: EventData<'_>) -> Result<Option<PeerEventCase>, PeerEventPayloadError> {
    let Some(event) = decode_event_case::<v1alpha1::PeerEnrolled, PeerEventCase>(&event)
        .or_else(|| decode_event_case::<v1alpha1::PeerAttached, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerHeartbeatRecorded, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerDetached, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerMessageAccepted, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerMessageIntakeRecorded, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerBatchPrepared, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerBatchAcknowledged, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerMessageDisposed, PeerEventCase>(&event))
        .or_else(|| decode_event_case::<v1alpha1::PeerMessageSettled, PeerEventCase>(&event))
    else {
        return Ok(None);
    };

    event.map(Some).map_err(PeerEventPayloadError::Decode)
}

fn peer_event_case_type(event: &PeerEventCase) -> &'static str {
    match event {
        PeerEventCase::PeerEnrolled(_) => event_type::<v1alpha1::PeerEnrolled>(),
        PeerEventCase::PeerAttached(_) => event_type::<v1alpha1::PeerAttached>(),
        PeerEventCase::PeerHeartbeatRecorded(_) => event_type::<v1alpha1::PeerHeartbeatRecorded>(),
        PeerEventCase::PeerDetached(_) => event_type::<v1alpha1::PeerDetached>(),
        PeerEventCase::PeerMessageAccepted(_) => event_type::<v1alpha1::PeerMessageAccepted>(),
        PeerEventCase::PeerMessageIntakeRecorded(_) => event_type::<v1alpha1::PeerMessageIntakeRecorded>(),
        PeerEventCase::PeerBatchPrepared(_) => event_type::<v1alpha1::PeerBatchPrepared>(),
        PeerEventCase::PeerBatchAcknowledged(_) => event_type::<v1alpha1::PeerBatchAcknowledged>(),
        PeerEventCase::PeerMessageDisposed(_) => event_type::<v1alpha1::PeerMessageDisposed>(),
        PeerEventCase::PeerMessageSettled(_) => event_type::<v1alpha1::PeerMessageSettled>(),
    }
}

#[cfg(test)]
mod tests;
