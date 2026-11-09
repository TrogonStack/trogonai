use buffa::Message as _;
use trogon_decider_runtime::{
    EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity, EventPayloadError, EventType,
    InvalidSnapshotTypeNameError, SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
    SnapshotTypeName,
};

use super::{CredentialEventCase, state_v1, v1};
use crate::codec::{decode_event_case, event_type, snapshot_type as proto_snapshot_type};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CredentialStateSnapshotPayloadError(#[source] buffa::DecodeError);

pub type CredentialEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1::CredentialEvent {
    type Error = CredentialEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        self.event
            .as_ref()
            .map(encode_credential_event_case)
            .ok_or(CredentialEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1::CredentialEvent {
    type Error = CredentialEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match decode_credential_event_case(event)? {
            Some(event) => Ok(EventDecodeOutcome::Decoded(v1::CredentialEvent { event: Some(event) })),
            None => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl EventIdentity for v1::CredentialEvent {}

impl EventType for v1::CredentialEvent {
    type Error = CredentialEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        self.event
            .as_ref()
            .map(credential_event_case_type)
            .ok_or(CredentialEventPayloadError::MissingEvent)
    }
}

fn encode_credential_event_case(event: &CredentialEventCase) -> Vec<u8> {
    match event {
        CredentialEventCase::WriteRequested(inner) => inner.encode_to_vec(),
        CredentialEventCase::WriteFailed(inner) => inner.encode_to_vec(),
        CredentialEventCase::Activated(inner) => inner.encode_to_vec(),
        CredentialEventCase::RotationRequested(inner) => inner.encode_to_vec(),
        CredentialEventCase::RotationFailed(inner) => inner.encode_to_vec(),
        CredentialEventCase::Rotated(inner) => inner.encode_to_vec(),
        CredentialEventCase::Revoked(inner) => inner.encode_to_vec(),
        CredentialEventCase::DestroyRequested(inner) => inner.encode_to_vec(),
        CredentialEventCase::Destroyed(inner) => inner.encode_to_vec(),
        CredentialEventCase::DestroyFailed(inner) => inner.encode_to_vec(),
    }
}

fn decode_credential_event_case(
    event: EventData<'_>,
) -> Result<Option<CredentialEventCase>, CredentialEventPayloadError> {
    let Some(event) = decode_event_case::<v1::CredentialWriteRequested, CredentialEventCase>(&event)
        .or_else(|| decode_event_case::<v1::CredentialWriteFailed, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialActivated, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialRotationRequested, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialRotationFailed, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialRotated, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialRevoked, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialDestroyRequested, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialDestroyed, CredentialEventCase>(&event))
        .or_else(|| decode_event_case::<v1::CredentialDestroyFailed, CredentialEventCase>(&event))
    else {
        return Ok(None);
    };

    event.map(Some).map_err(CredentialEventPayloadError::Decode)
}

fn credential_event_case_type(event: &CredentialEventCase) -> &'static str {
    match event {
        CredentialEventCase::WriteRequested(_) => event_type::<v1::CredentialWriteRequested>(),
        CredentialEventCase::WriteFailed(_) => event_type::<v1::CredentialWriteFailed>(),
        CredentialEventCase::Activated(_) => event_type::<v1::CredentialActivated>(),
        CredentialEventCase::RotationRequested(_) => event_type::<v1::CredentialRotationRequested>(),
        CredentialEventCase::RotationFailed(_) => event_type::<v1::CredentialRotationFailed>(),
        CredentialEventCase::Rotated(_) => event_type::<v1::CredentialRotated>(),
        CredentialEventCase::Revoked(_) => event_type::<v1::CredentialRevoked>(),
        CredentialEventCase::DestroyRequested(_) => event_type::<v1::CredentialDestroyRequested>(),
        CredentialEventCase::Destroyed(_) => event_type::<v1::CredentialDestroyed>(),
        CredentialEventCase::DestroyFailed(_) => event_type::<v1::CredentialDestroyFailed>(),
    }
}

impl SnapshotType for state_v1::CredentialStateSnapshot {
    type Error = InvalidSnapshotTypeNameError;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        proto_snapshot_type::<Self>()
    }
}

impl SnapshotPayloadEncode for state_v1::CredentialStateSnapshot {
    type Error = std::convert::Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.encode_to_vec())
    }
}

impl SnapshotPayloadDecode for state_v1::CredentialStateSnapshot {
    type Error = CredentialStateSnapshotPayloadError;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        Self::decode_from_slice(payload.payload).map_err(CredentialStateSnapshotPayloadError)
    }
}

#[cfg(test)]
mod tests;
