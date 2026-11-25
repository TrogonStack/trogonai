use buffa::{Message as _, MessageField};
use trogon_decider_runtime::{
    EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, SnapshotPayloadData, SnapshotPayloadDecode,
    SnapshotPayloadEncode,
};

use super::*;

const CREDENTIAL_ID: &str = "6f9619ff-8b86-4011-b42d-00c04fc964ff";

fn identity() -> v1::CredentialIdentity {
    v1::CredentialIdentity {
        credential_id: CREDENTIAL_ID.to_string(),
        owner_id: "tenant-1".to_string(),
        source: Some(v1::CredentialSource::Github.into()),
        scope_key: "github".to_string(),
        kind: Some(v1::CredentialKind::WebhookSecret.into()),
        identity_version: Some(v1::CredentialIdentityVersion::V1.into()),
    }
}

fn key() -> v1::CredentialKey {
    v1::CredentialKey {
        credential_id: CREDENTIAL_ID.to_string(),
        owner_id: "tenant-1".to_string(),
    }
}

fn write_requested() -> v1::CredentialWriteRequested {
    v1::CredentialWriteRequested {
        identity: MessageField::some(identity()),
    }
}

fn decode(event_type: &str, payload: &[u8]) -> v1::CredentialEvent {
    <v1::CredentialEvent as EventDecode>::decode(EventData::new(event_type, payload))
        .unwrap()
        .into_decoded()
        .unwrap()
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

    let decoded = decode(
        <v1::CredentialWriteRequested as buffa::MessageName>::FULL_NAME,
        &inner.encode_to_vec(),
    );

    assert_eq!(decoded.event, Some(CredentialEventCase::from(inner)));
}

#[test]
fn event_decode_dispatches_all_lifecycle_event_types() {
    let activated = v1::CredentialActivated {
        key: key().into(),
        version: Some(1),
        fingerprint: "fingerprint".to_string(),
    };
    let revoked = v1::CredentialRevoked {
        key: key().into(),
        version: Some(1),
    };
    let destroy_requested = v1::CredentialDestroyRequested {
        key: key().into(),
        version: Some(1),
        reason: "logical cleanup complete".to_string(),
    };
    let destroyed = v1::CredentialDestroyed {
        key: key().into(),
        version: Some(1),
    };
    let destroy_failed = v1::CredentialDestroyFailed {
        key: key().into(),
        version: Some(1),
        reason: "secret store unavailable".to_string(),
    };

    assert_eq!(
        decode(
            <v1::CredentialActivated as buffa::MessageName>::FULL_NAME,
            &activated.encode_to_vec()
        )
        .event,
        Some(CredentialEventCase::from(activated))
    );
    assert_eq!(
        decode(
            <v1::CredentialRevoked as buffa::MessageName>::FULL_NAME,
            &revoked.encode_to_vec()
        )
        .event,
        Some(CredentialEventCase::from(revoked))
    );
    assert_eq!(
        decode(
            <v1::CredentialDestroyRequested as buffa::MessageName>::FULL_NAME,
            &destroy_requested.encode_to_vec()
        )
        .event,
        Some(CredentialEventCase::from(destroy_requested))
    );
    assert_eq!(
        decode(
            <v1::CredentialDestroyed as buffa::MessageName>::FULL_NAME,
            &destroyed.encode_to_vec()
        )
        .event,
        Some(CredentialEventCase::from(destroyed))
    );
    assert_eq!(
        decode(
            <v1::CredentialDestroyFailed as buffa::MessageName>::FULL_NAME,
            &destroy_failed.encode_to_vec()
        )
        .event,
        Some(CredentialEventCase::from(destroy_failed))
    );
}

#[test]
fn event_decode_skips_unknown_event_type() {
    assert!(matches!(
        <v1::CredentialEvent as EventDecode>::decode(EventData::new("trogonai.gateway.credentials.v1.Unknown", &[])),
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
                key: MessageField::none(),
                version: None,
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
    let error = <state_v1::CredentialStateSnapshot as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(b"\0"))
        .unwrap_err();

    assert!(!error.to_string().is_empty());
    assert!(std::error::Error::source(&error).is_some());
}
