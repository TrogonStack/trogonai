use buffa::Message as _;
use trogon_decider_runtime::{
    EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity, EventPayloadError, EventType,
    InvalidSnapshotTypeName, SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
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
    }
}

impl SnapshotType for state_v1::CredentialStateSnapshot {
    type Error = InvalidSnapshotTypeName;

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
mod tests {
    use buffa::{Message as _, MessageField};
    use trogon_decider_runtime::{
        EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, SnapshotPayloadData, SnapshotPayloadDecode,
        SnapshotPayloadEncode,
    };

    use super::*;

    fn write_requested() -> v1::CredentialWriteRequested {
        v1::CredentialWriteRequested {
            credential_id: "github:primary:webhook_secret".to_string(),
            owner_id: "tenant-1".to_string(),
            source: Some(v1::CredentialSource::Github.into()),
            kind: Some(v1::CredentialKind::WebhookSecret.into()),
        }
    }

    #[test]
    fn event_encode_writes_inner_event_payload() {
        let inner = write_requested();
        let event = v1::CredentialEvent {
            event: Some(inner.clone().into()),
        };

        let encoded = EventEncode::encode(&event).unwrap();

        assert_eq!(
            v1::CredentialWriteRequested::decode_from_slice(&encoded).unwrap(),
            inner
        );
    }

    #[test]
    fn event_encode_rejects_missing_event_case() {
        let event = v1::CredentialEvent { event: None };

        assert!(matches!(
            EventEncode::encode(&event),
            Err(CredentialEventPayloadError::MissingEvent)
        ));
    }

    #[test]
    fn event_decode_dispatches_by_generated_full_name() {
        let inner = write_requested();
        let encoded = inner.encode_to_vec();

        let decoded = <v1::CredentialEvent as EventDecode>::decode(EventData::new(
            <v1::CredentialWriteRequested as buffa::MessageName>::FULL_NAME,
            &encoded,
        ))
        .unwrap();

        let decoded = decoded.into_decoded().unwrap();
        assert!(matches!(decoded.event, Some(CredentialEventCase::WriteRequested(_))));
    }

    #[test]
    fn event_decode_dispatches_all_lifecycle_event_types() {
        let activated = v1::CredentialActivated {
            metadata: MessageField::some(v1::CredentialMetadata {
                reference: MessageField::some(v1::CredentialRef {
                    id: "github:primary:webhook_secret".to_string(),
                    version: Some(1),
                    owner_id: "tenant-1".to_string(),
                    source: Some(v1::CredentialSource::Github.into()),
                    scope_key: "github".to_string(),
                    kind: Some(v1::CredentialKind::WebhookSecret.into()),
                }),
                status: Some(v1::CredentialStatus::Active.into()),
                storage_backend: Some(v1::StorageBackend::Openbao.into()),
                fingerprint: "fingerprint".to_string(),
            }),
        };
        let revoked = v1::CredentialRevoked {
            credential_ref: MessageField::some(v1::CredentialRef {
                id: "github:primary:webhook_secret".to_string(),
                version: Some(1),
                owner_id: "tenant-1".to_string(),
                source: Some(v1::CredentialSource::Github.into()),
                scope_key: "github".to_string(),
                kind: Some(v1::CredentialKind::WebhookSecret.into()),
            }),
        };

        let decoded = <v1::CredentialEvent as EventDecode>::decode(EventData::new(
            <v1::CredentialActivated as buffa::MessageName>::FULL_NAME,
            &activated.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(decoded.event, Some(CredentialEventCase::Activated(_))));

        let decoded = <v1::CredentialEvent as EventDecode>::decode(EventData::new(
            <v1::CredentialRevoked as buffa::MessageName>::FULL_NAME,
            &revoked.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(decoded.event, Some(CredentialEventCase::Revoked(_))));
    }

    #[test]
    fn event_decode_skips_unknown_event_type() {
        assert!(matches!(
            <v1::CredentialEvent as EventDecode>::decode(EventData::new(
                "trogonai.gateway.credentials.v1.Unknown",
                &[]
            )),
            Ok(EventDecodeOutcome::Skipped)
        ));
    }

    #[test]
    fn event_decode_preserves_payload_decode_errors() {
        assert!(matches!(
            <v1::CredentialEvent as EventDecode>::decode(EventData::new(
                <v1::CredentialRevoked as buffa::MessageName>::FULL_NAME,
                b"\0"
            )),
            Err(CredentialEventPayloadError::Decode(_))
        ));
    }

    #[test]
    fn event_type_returns_inner_event_full_name() {
        let event = v1::CredentialEvent {
            event: Some(
                v1::CredentialRevoked {
                    credential_ref: MessageField::none(),
                }
                .into(),
            ),
        };

        assert_eq!(
            event.event_type().unwrap(),
            <v1::CredentialRevoked as buffa::MessageName>::FULL_NAME
        );
    }

    #[test]
    fn event_type_rejects_missing_event_case() {
        let event = v1::CredentialEvent { event: None };

        assert!(matches!(
            event.event_type(),
            Err(CredentialEventPayloadError::MissingEvent)
        ));
    }

    #[test]
    fn state_snapshot_round_trips_through_snapshot_traits() {
        let state = state_v1::CredentialStateSnapshot {
            state: Some(state_v1::CredentialMissingState {}.into()),
        };

        let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
        let decoded =
            <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded))
                .unwrap();

        assert_eq!(decoded, state);
    }

    #[test]
    fn state_snapshot_type_uses_generated_full_name() {
        assert_eq!(
            <state_v1::CredentialStateSnapshot as SnapshotType>::snapshot_type()
                .unwrap()
                .as_str(),
            <state_v1::CredentialStateSnapshot as buffa::MessageName>::FULL_NAME
        );
    }

    #[test]
    fn state_snapshot_decode_preserves_payload_decode_errors() {
        let error =
            <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(b"\0"))
                .unwrap_err();

        assert!(!error.to_string().is_empty());
        assert!(std::error::Error::source(&error).is_some());
    }
}
