use buffa::{EnumValue, Enumeration, Message};
use serde_json::{Value, json};

use super::{assert_json_codec, assert_proto_sequence, assert_wire_codec};
use crate::gateway::credentials::{
    CredentialEventCase, CredentialStateSnapshotCase, checkpoints_v1, idempotency_v1, state_v1, v1,
};

pub(super) fn credential_ref() -> Value {
    json!({
        "id": "cred-1", "version": "2", "ownerId": "owner-1", "source": "CREDENTIAL_SOURCE_GITHUB",
        "scopeKey": "org/repo", "kind": "CREDENTIAL_KIND_WEBHOOK_SECRET"
    })
}

fn merged(mut base: Value, extra: Value) -> Value {
    base.as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    base
}

pub(super) fn ref_with(extra: Value) -> Value {
    merged(json!({"id": "", "ownerId": "", "scopeKey": ""}), extra)
}

pub(super) fn metadata_with(extra: Value) -> Value {
    merged(json!({"reference": ref_with(json!({})), "fingerprint": ""}), extra)
}

pub(super) fn metadata() -> Value {
    json!({
        "reference": credential_ref(), "status": "CREDENTIAL_STATUS_ACTIVE", "fingerprint": "sha256:abc"
    })
}

pub(super) fn state_metadata() -> Value {
    json!({
        "reference": credential_ref(), "status": "CREDENTIAL_STATUS_ACTIVE",
        "storageBackend": "STORAGE_BACKEND_OPENBAO", "fingerprint": "sha256:abc"
    })
}

pub(super) fn active_state() -> Value {
    json!({
        "metadata": state_metadata(),
        "previousVersions": [credential_ref(), {"id": "cred-1", "version": "1", "ownerId": "", "scopeKey": ""}]
    })
}

pub(super) fn events() -> [Value; 10] {
    [
        json!({"writeRequested": {
            "credentialId": "cred-1", "ownerId": "owner-1", "source": "CREDENTIAL_SOURCE_SLACK",
            "kind": "CREDENTIAL_KIND_BOT_TOKEN", "scopeKey": "team"
        }}),
        json!({"writeFailed": {"credentialId": "cred-1", "reason": "vault down"}}),
        json!({"activated": {"metadata": metadata()}}),
        json!({"rotationRequested": {"credentialRef": credential_ref()}}),
        json!({"rotationFailed": {"credentialRef": credential_ref(), "reason": "conflict"}}),
        json!({"rotated": {"previousCredentialRef": credential_ref(), "metadata": metadata()}}),
        json!({"revoked": {"credentialRef": credential_ref()}}),
        json!({"destroyRequested": {"credentialRef": credential_ref(), "reason": "expired"}}),
        json!({"destroyed": {"credentialRef": credential_ref()}}),
        json!({"destroyFailed": {"credentialRef": credential_ref(), "reason": "timeout"}}),
    ]
}

pub(super) fn snapshots() -> [Value; 9] {
    [
        json!({"missing": {}}),
        json!({"pendingWrite": {
            "credentialId": "cred-1", "ownerId": "owner-1", "source": "CREDENTIAL_SOURCE_ZOOM",
            "kind": "CREDENTIAL_KIND_CLIENT_SECRET", "scopeKey": "acct"
        }}),
        json!({"active": active_state()}),
        json!({"writeFailed": {"credentialId": "cred-1", "reason": "denied"}}),
        json!({"rotationPending": {"active": active_state()}}),
        json!({"revoked": {"credentialRef": credential_ref()}}),
        json!({"destroyRequested": {"credentialRef": credential_ref(), "reason": "policy"}}),
        json!({"destroyed": {"credentialRef": credential_ref()}}),
        json!({"cleanupFailed": {"credentialRef": credential_ref(), "reason": "orphaned"}}),
    ]
}

pub(super) fn command_response() -> Value {
    json!({
        "state": "active", "streamPosition": "18446744073709551615",
        "credentialRef": {
            "id": "cred-1", "version": "3", "ownerId": "owner-1", "source": "github",
            "scopeKey": "org/repo", "kind": "webhook_secret", "status": "active"
        }
    })
}

#[test]
fn credential_reference_and_metadata_keep_wire_and_json_contracts() {
    let reference = assert_json_codec::<v1::CredentialRef>(credential_ref());
    assert_eq!(reference.version, Some(2));
    assert_json_codec::<v1::CredentialMetadata>(metadata());
    assert_json_codec::<state_v1::CredentialMetadata>(state_metadata());
    assert_json_codec::<v1::CredentialRef>(ref_with(json!({"version": "18446744073709551615"})));
}

#[test]
fn every_enum_value_round_trips_through_names_numbers_and_unknown_values() {
    for source in v1::CredentialSource::values() {
        let json = ref_with(json!({"id": "c", "source": source.proto_name()}));
        let message = assert_json_codec::<v1::CredentialRef>(json);
        assert_eq!(message.source, Some(EnumValue::from(*source)));
        assert_eq!(v1::CredentialSource::from_i32(source.to_i32()), Some(*source));
        assert_eq!(
            v1::CredentialSource::from_proto_name(source.proto_name()),
            Some(*source)
        );
    }
    for kind in v1::CredentialKind::values() {
        let message = assert_json_codec::<v1::CredentialRef>(ref_with(json!({"id": "c", "kind": kind.proto_name()})));
        assert_eq!(message.kind, Some(EnumValue::from(*kind)));
        assert_eq!(v1::CredentialKind::from_i32(kind.to_i32()), Some(*kind));
        assert_eq!(v1::CredentialKind::from_proto_name(kind.proto_name()), Some(*kind));
    }
    for status in v1::CredentialStatus::values() {
        let message =
            assert_json_codec::<v1::CredentialMetadata>(metadata_with(json!({"status": status.proto_name()})));
        assert_eq!(message.status, Some(EnumValue::from(*status)));
        assert_eq!(v1::CredentialStatus::from_i32(status.to_i32()), Some(*status));
        assert_eq!(
            v1::CredentialStatus::from_proto_name(status.proto_name()),
            Some(*status)
        );
    }
    for backend in state_v1::StorageBackend::values() {
        let message = assert_json_codec::<state_v1::CredentialMetadata>(metadata_with(
            json!({"storageBackend": backend.proto_name()}),
        ));
        assert_eq!(message.storage_backend, Some(EnumValue::from(*backend)));
        assert_eq!(state_v1::StorageBackend::from_i32(backend.to_i32()), Some(*backend));
        assert_eq!(
            state_v1::StorageBackend::from_proto_name(backend.proto_name()),
            Some(*backend)
        );
    }
    for status in idempotency_v1::CredentialManagementIdempotencyStatus::values() {
        let message = assert_json_codec::<idempotency_v1::CredentialManagementIdempotencyRecord>(
            json!({"fingerprint": "f", "status": status.proto_name()}),
        );
        assert_eq!(message.status, Some(EnumValue::from(*status)));
        assert_eq!(
            idempotency_v1::CredentialManagementIdempotencyStatus::from_i32(status.to_i32()),
            Some(*status)
        );
        assert_eq!(
            idempotency_v1::CredentialManagementIdempotencyStatus::from_proto_name(status.proto_name()),
            Some(*status)
        );
    }
    assert_eq!(v1::CredentialSource::from_i32(99), None);
    assert_eq!(v1::CredentialKind::from_proto_name("CREDENTIAL_KIND_FUTURE"), None);
    assert_json_codec::<v1::CredentialRef>(ref_with(json!({"id": "c", "source": 99, "kind": 99})));
    assert_json_codec::<v1::CredentialMetadata>(metadata_with(json!({"status": 99})));
    assert_json_codec::<state_v1::CredentialMetadata>(metadata_with(json!({"status": 99, "storageBackend": 99})));
}

#[test]
fn lifecycle_events_cover_every_oneof_variant() {
    for event in events() {
        assert_json_codec::<v1::CredentialEvent>(event);
    }
    assert_json_codec::<v1::CredentialEvent>(json!({}));
    let parsed: Vec<v1::CredentialEvent> = events()
        .into_iter()
        .map(|event| serde_json::from_value(event).expect("event"))
        .collect();
    assert_proto_sequence(parsed, json!(events()));
}

#[test]
fn event_payload_messages_keep_wire_and_json_contracts() {
    let [
        write_requested,
        write_failed,
        activated,
        rotation_requested,
        rotation_failed,
        rotated,
        revoked,
        destroy_requested,
        destroyed,
        destroy_failed,
    ] = events().map(|mut event| {
        event
            .as_object_mut()
            .expect("event")
            .values_mut()
            .next()
            .expect("payload")
            .take()
    });
    assert_json_codec::<v1::CredentialWriteRequested>(write_requested);
    assert_json_codec::<v1::CredentialWriteFailed>(write_failed);
    assert_json_codec::<v1::CredentialActivated>(activated);
    assert_json_codec::<v1::CredentialRotationRequested>(rotation_requested);
    assert_json_codec::<v1::CredentialRotationFailed>(rotation_failed);
    assert_json_codec::<v1::CredentialRotated>(rotated);
    assert_json_codec::<v1::CredentialRevoked>(revoked);
    assert_json_codec::<v1::CredentialDestroyRequested>(destroy_requested);
    assert_json_codec::<v1::CredentialDestroyed>(destroyed);
    assert_json_codec::<v1::CredentialDestroyFailed>(destroy_failed);
}

#[test]
fn event_oneof_switches_to_the_last_variant_seen_on_the_wire() {
    let first: v1::CredentialEvent = serde_json::from_value(events()[0].clone()).expect("first");
    let last: v1::CredentialEvent = serde_json::from_value(events()[9].clone()).expect("last");
    let wire = [first.encode_to_vec(), last.encode_to_vec()].concat();
    assert_wire_codec(&wire, &last);
    assert!(matches!(last.event, Some(CredentialEventCase::DestroyFailed(_))));
    let mut reused = first;
    reused.event = Some(CredentialEventCase::from(v1::CredentialRevoked::default()));
    assert!(matches!(reused.event, Some(CredentialEventCase::Revoked(_))));
    reused.clear();
    assert!(reused.event.is_none());
}

#[test]
fn repeated_event_messages_merge_when_the_same_variant_repeats() {
    let wire = b"\x0a\x08\x0a\x06cred-1\x0a\x04\x12\x02ow";
    let expected: v1::CredentialEvent = serde_json::from_value(json!({
        "writeRequested": {"credentialId": "cred-1", "ownerId": "ow"}
    }))
    .expect("merged event");
    assert_wire_codec(wire, &expected);
}

#[test]
fn state_snapshot_covers_every_oneof_variant() {
    for snapshot in snapshots() {
        assert_json_codec::<state_v1::CredentialStateSnapshot>(snapshot);
    }
    assert_json_codec::<state_v1::CredentialStateSnapshot>(json!({}));
    let parsed: Vec<state_v1::CredentialStateSnapshot> = snapshots()
        .into_iter()
        .map(|snapshot| serde_json::from_value(snapshot).expect("snapshot"))
        .collect();
    assert_proto_sequence(parsed.clone(), json!(snapshots()));
    let wire = [parsed[0].encode_to_vec(), parsed[8].encode_to_vec()].concat();
    assert_wire_codec(&wire, &parsed[8]);
    assert!(matches!(
        parsed[8].state,
        Some(CredentialStateSnapshotCase::CleanupFailed(_))
    ));
    let mut reused = parsed[1].clone();
    reused.state = Some(CredentialStateSnapshotCase::from(
        state_v1::CredentialMissingState::default(),
    ));
    assert!(matches!(reused.state, Some(CredentialStateSnapshotCase::Missing(_))));
    reused.clear();
    assert!(reused.state.is_none());
}

#[test]
fn state_payload_messages_keep_wire_and_json_contracts() {
    assert_json_codec::<state_v1::CredentialMissingState>(json!({}));
    let [
        _,
        pending,
        active,
        failed,
        rotation,
        revoked,
        destroy_requested,
        destroyed,
        cleanup,
    ] = snapshots().map(|mut snapshot| {
        snapshot
            .as_object_mut()
            .expect("snapshot")
            .values_mut()
            .next()
            .expect("payload")
            .take()
    });
    assert_json_codec::<state_v1::PendingCredentialWriteState>(pending);
    let active = assert_json_codec::<state_v1::ActiveCredentialState>(active);
    assert_eq!(active.previous_versions.len(), 2);
    assert_json_codec::<state_v1::FailedCredentialWriteState>(failed);
    assert_json_codec::<state_v1::RotationPendingCredentialState>(rotation);
    assert_json_codec::<state_v1::RevokedCredentialState>(revoked);
    assert_json_codec::<state_v1::DestroyRequestedCredentialState>(destroy_requested);
    assert_json_codec::<state_v1::DestroyedCredentialState>(destroyed);
    assert_json_codec::<state_v1::CleanupFailedCredentialState>(cleanup);
}

#[test]
fn checkpoints_keep_uint64_precision_and_zero_values() {
    let projection = assert_json_codec::<checkpoints_v1::RuntimeProjectionCheckpoint>(
        json!({"lastScannedSequence": "18446744073709551615"}),
    );
    assert_eq!(projection.last_scanned_sequence, Some(u64::MAX));
    let worker = assert_json_codec::<checkpoints_v1::CredentialRecoveryWorkerCheckpoint>(json!({
        "lastScannedSequence": "9007199254740993", "consecutiveFailureCount": 4294967295_u32,
        "firstFailureUnixSeconds": "1700000000", "retryAfterUnixSeconds": "1700000060"
    }));
    assert_eq!(worker.consecutive_failure_count, Some(u32::MAX));
}

#[test]
fn idempotency_records_cover_pending_and_completed_responses() {
    let response = assert_json_codec::<idempotency_v1::CredentialCommandResponse>(command_response());
    assert_eq!(response.stream_position, Some(u64::MAX));
    assert_json_codec::<idempotency_v1::CredentialRefResponse>(command_response()["credentialRef"].clone());
    assert_json_codec::<idempotency_v1::CredentialManagementIdempotencyRecord>(json!({
        "fingerprint": "fp", "status": "CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_IN_PROGRESS"
    }));
    assert_json_codec::<idempotency_v1::CredentialManagementIdempotencyRecord>(json!({
        "fingerprint": "fp", "status": "CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED",
        "response": command_response()
    }));
}
