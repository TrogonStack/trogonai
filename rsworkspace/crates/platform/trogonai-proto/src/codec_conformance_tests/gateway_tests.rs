use buffa::{EnumValue, Enumeration, Message};
use serde_json::{Value, json};

use super::{assert_json_codec, assert_proto_sequence, assert_wire_codec};
use crate::gateway::credentials::{
    CredentialEventCase, CredentialStateSnapshotCase, checkpoints_v1, idempotency_v1, state_v1, v1,
};

pub(super) const CREDENTIAL_ID: &str = "6f9619ff-8b86-4011-b42d-00c04fc964ff";

pub(super) fn identity() -> Value {
    json!({
        "credentialId": CREDENTIAL_ID, "ownerId": "owner-1", "source": "CREDENTIAL_SOURCE_GITHUB",
        "scopeKey": "github", "kind": "CREDENTIAL_KIND_WEBHOOK_SECRET",
        "identityVersion": "CREDENTIAL_IDENTITY_VERSION_V1"
    })
}

fn merged(mut base: Value, extra: Value) -> Value {
    base.as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    base
}

pub(super) fn identity_with(extra: Value) -> Value {
    merged(json!({"credentialId": "", "ownerId": "", "scopeKey": ""}), extra)
}

pub(super) fn key() -> Value {
    json!({"credentialId": CREDENTIAL_ID, "ownerId": "owner-1"})
}

pub(super) fn active_state() -> Value {
    json!({
        "identity": identity(), "version": "3", "fingerprint": "sha256:abc", "previousVersions": ["1", "2"]
    })
}

pub(super) fn events() -> [Value; 10] {
    [
        json!({"writeRequested": {"identity": identity()}}),
        json!({"writeFailed": {"key": key(), "reason": "vault down"}}),
        json!({"activated": {"key": key(), "version": "1", "fingerprint": "sha256:abc"}}),
        json!({"rotationRequested": {"key": key(), "version": "1"}}),
        json!({"rotationFailed": {"key": key(), "version": "1", "reason": "conflict"}}),
        json!({"rotated": {"key": key(), "previousVersion": "1", "version": "2", "fingerprint": "sha256:def"}}),
        json!({"revoked": {"key": key(), "version": "2"}}),
        json!({"destroyRequested": {"key": key(), "version": "2", "reason": "expired"}}),
        json!({"destroyed": {"key": key(), "version": "2"}}),
        json!({"destroyFailed": {"key": key(), "version": "2", "reason": "timeout"}}),
    ]
}

pub(super) fn snapshots() -> [Value; 9] {
    [
        json!({"missing": {}}),
        json!({"pendingWrite": {"identity": identity_with(json!({
            "credentialId": CREDENTIAL_ID, "ownerId": "owner-1", "source": "CREDENTIAL_SOURCE_ZOOM",
            "kind": "CREDENTIAL_KIND_CLIENT_SECRET", "scopeKey": "zoom"
        }))}}),
        json!({"active": active_state()}),
        json!({"writeFailed": {"identity": identity(), "reason": "denied"}}),
        json!({"rotationPending": {"active": active_state()}}),
        json!({"revoked": {"identity": identity(), "version": "3"}}),
        json!({"destroyRequested": {"identity": identity(), "version": "3", "reason": "policy"}}),
        json!({"destroyed": {"identity": identity(), "version": "3"}}),
        json!({"cleanupFailed": {"identity": identity(), "version": "3", "reason": "orphaned"}}),
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
fn credential_identity_and_key_keep_wire_and_json_contracts() {
    let identity = assert_json_codec::<v1::CredentialIdentity>(identity());
    assert_eq!(
        identity.identity_version,
        Some(EnumValue::from(v1::CredentialIdentityVersion::V1))
    );
    assert_json_codec::<v1::CredentialIdentity>(identity_with(json!({})));
    assert_json_codec::<v1::CredentialKey>(key());
    assert_json_codec::<v1::CredentialKey>(json!({"credentialId": "", "ownerId": ""}));
    let revoked = assert_json_codec::<v1::CredentialRevoked>(json!({"key": key(), "version": "18446744073709551615"}));
    assert_eq!(revoked.version, Some(u64::MAX));
}

#[test]
fn every_enum_value_round_trips_through_names_numbers_and_unknown_values() {
    for source in v1::CredentialSource::values() {
        let message =
            assert_json_codec::<v1::CredentialIdentity>(identity_with(json!({"source": source.proto_name()})));
        assert_eq!(message.source, Some(EnumValue::from(*source)));
        assert_eq!(v1::CredentialSource::from_i32(source.to_i32()), Some(*source));
        assert_eq!(
            v1::CredentialSource::from_proto_name(source.proto_name()),
            Some(*source)
        );
    }
    for kind in v1::CredentialKind::values() {
        let message = assert_json_codec::<v1::CredentialIdentity>(identity_with(json!({"kind": kind.proto_name()})));
        assert_eq!(message.kind, Some(EnumValue::from(*kind)));
        assert_eq!(v1::CredentialKind::from_i32(kind.to_i32()), Some(*kind));
        assert_eq!(v1::CredentialKind::from_proto_name(kind.proto_name()), Some(*kind));
    }
    for version in v1::CredentialIdentityVersion::values() {
        let message = assert_json_codec::<v1::CredentialIdentity>(identity_with(
            json!({"identityVersion": version.proto_name()}),
        ));
        assert_eq!(message.identity_version, Some(EnumValue::from(*version)));
        assert_eq!(
            v1::CredentialIdentityVersion::from_i32(version.to_i32()),
            Some(*version)
        );
        assert_eq!(
            v1::CredentialIdentityVersion::from_proto_name(version.proto_name()),
            Some(*version)
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
    assert_eq!(v1::CredentialIdentityVersion::from_i32(99), None);
    assert_json_codec::<v1::CredentialIdentity>(identity_with(
        json!({"source": 99, "kind": 99, "identityVersion": 99}),
    ));
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
    let wire = b"\x0a\x0a\x0a\x08\x0a\x06cred-1\x0a\x06\x0a\x04\x12\x02ow";
    let expected: v1::CredentialEvent = serde_json::from_value(json!({
        "writeRequested": {"identity": {"credentialId": "cred-1", "ownerId": "ow"}}
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
    assert_eq!(active.previous_versions, [1, 2]);
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
