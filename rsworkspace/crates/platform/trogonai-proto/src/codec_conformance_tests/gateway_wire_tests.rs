use std::fmt::Debug;

use buffa::{DecodeError, HasMessageView, Message, MessageView};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::gateway_tests::{command_response, credential_ref, events, metadata, snapshots, state_metadata};
use super::{assert_malformed, assert_wire_codec};
use crate::gateway::credentials::{
    CredentialEventCase, CredentialStateSnapshotCase, checkpoints_v1, idempotency_v1, state_v1, v1,
};

fn length_prefixed(field: u8, inner: &[u8]) -> Vec<u8> {
    let mut wire = vec![field << 3 | 2, u8::try_from(inner.len()).expect("short fixture")];
    wire.extend_from_slice(inner);
    wire
}

fn assert_wire_types<M: Message + HasMessageView>(length_delimited: &[u8], varints: &[u8]) {
    for (expected, fields) in [(2_u8, length_delimited), (0, varints)] {
        let actual = if expected == 2 { 0 } else { 2 };
        for &field in fields {
            assert_malformed::<M>(
                &[field << 3 | actual, 0],
                DecodeError::WireTypeMismatch {
                    field_number: u32::from(field),
                    expected,
                    actual,
                },
            );
            let incomplete = [field << 3 | expected, if expected == 2 { 2 } else { 0x80 }];
            assert_malformed::<M>(&incomplete, DecodeError::UnexpectedEof);
        }
    }
}

fn assert_invalid_utf8<M: Message + HasMessageView>(fields: &[u8]) {
    for &field in fields {
        assert_malformed::<M>(&length_prefixed(field, &[0xff]), DecodeError::InvalidUtf8);
    }
}

fn assert_nested_invalid_utf8<M: Message + HasMessageView>(fields: &[u8], inner: &[u8]) {
    for &field in fields {
        assert_malformed::<M>(&length_prefixed(field, inner), DecodeError::InvalidUtf8);
    }
}

fn assert_normalized<M: DeserializeOwned + Serialize>(input: Value, expected: Value) {
    let parsed: M = serde_json::from_value(input).expect("accepted ProtoJSON");
    assert_eq!(serde_json::to_value(parsed).expect("canonical ProtoJSON"), expected);
}

fn assert_rejected<M: DeserializeOwned + Debug>(input: &str) {
    assert!(serde_json::from_str::<M>(input).is_err(), "accepted {input}");
}

macro_rules! assert_presence {
    ($message:ty, $present_wire:expr, $($has:ident),+ $(,)?) => {{
        let absent = <$message>::decode_view(b"").expect("absent required fields");
        let present = <$message>::decode_view($present_wire).expect("explicit default required fields");
        $(
            assert!(!absent.$has(), "absent field {}", stringify!($has));
            assert!(present.$has(), "present field {}", stringify!($has));
        )+
        assert_eq!(absent.to_owned_message().expect("owned absent"), <$message>::default());
    }};
}

#[test]
fn required_field_presence_is_tracked_independently_of_empty_values() {
    assert_presence!(
        v1::CredentialRef,
        b"\x0a\x00\x1a\x00\x2a\x00",
        has_id,
        has_owner_id,
        has_scope_key
    );
    assert_presence!(
        v1::CredentialMetadata,
        b"\x0a\x00\x22\x00",
        has_reference,
        has_fingerprint
    );
    assert_presence!(v1::CredentialActivated, b"\x0a\x00", has_metadata);
    assert_presence!(
        v1::CredentialDestroyFailed,
        b"\x0a\x00\x12\x00",
        has_credential_ref,
        has_reason
    );
    assert_presence!(
        v1::CredentialDestroyRequested,
        b"\x0a\x00\x12\x00",
        has_credential_ref,
        has_reason
    );
    assert_presence!(v1::CredentialDestroyed, b"\x0a\x00", has_credential_ref);
    assert_presence!(v1::CredentialRevoked, b"\x0a\x00", has_credential_ref);
    assert_presence!(
        v1::CredentialRotated,
        b"\x0a\x00\x12\x00",
        has_previous_credential_ref,
        has_metadata
    );
    assert_presence!(
        v1::CredentialRotationFailed,
        b"\x0a\x00\x12\x00",
        has_credential_ref,
        has_reason
    );
    assert_presence!(v1::CredentialRotationRequested, b"\x0a\x00", has_credential_ref);
    assert_presence!(
        v1::CredentialWriteFailed,
        b"\x0a\x00\x12\x00",
        has_credential_id,
        has_reason
    );
    assert_presence!(
        v1::CredentialWriteRequested,
        b"\x0a\x00\x12\x00\x2a\x00",
        has_credential_id,
        has_owner_id,
        has_scope_key
    );
    assert_presence!(
        state_v1::CredentialMetadata,
        b"\x0a\x00\x22\x00",
        has_reference,
        has_fingerprint
    );
    assert_presence!(
        state_v1::PendingCredentialWriteState,
        b"\x0a\x00\x12\x00\x2a\x00",
        has_credential_id,
        has_owner_id,
        has_scope_key
    );
    assert_presence!(state_v1::ActiveCredentialState, b"\x0a\x00", has_metadata);
    assert_presence!(
        state_v1::FailedCredentialWriteState,
        b"\x0a\x00\x12\x00",
        has_credential_id,
        has_reason
    );
    assert_presence!(state_v1::RotationPendingCredentialState, b"\x0a\x00", has_active);
    assert_presence!(state_v1::RevokedCredentialState, b"\x0a\x00", has_credential_ref);
    assert_presence!(
        state_v1::DestroyRequestedCredentialState,
        b"\x0a\x00\x12\x00",
        has_credential_ref,
        has_reason
    );
    assert_presence!(state_v1::DestroyedCredentialState, b"\x0a\x00", has_credential_ref);
    assert_presence!(
        state_v1::CleanupFailedCredentialState,
        b"\x0a\x00\x12\x00",
        has_credential_ref,
        has_reason
    );
    assert_presence!(
        idempotency_v1::CredentialManagementIdempotencyRecord,
        b"\x0a\x00",
        has_fingerprint
    );
    assert_presence!(
        idempotency_v1::CredentialCommandResponse,
        b"\x0a\x00\x1a\x00",
        has_state,
        has_credential_ref
    );
    assert_presence!(
        idempotency_v1::CredentialRefResponse,
        b"\x0a\x00\x1a\x00\x22\x00\x2a\x00\x32\x00\x3a\x00",
        has_id,
        has_owner_id,
        has_source,
        has_scope_key,
        has_kind,
        has_status
    );
}

#[test]
fn known_fields_reject_wrong_wire_types_and_truncated_values() {
    assert_wire_types::<v1::CredentialRef>(&[1, 3, 5], &[2, 4, 6]);
    assert_wire_types::<v1::CredentialMetadata>(&[1, 4], &[2]);
    assert_wire_types::<v1::CredentialActivated>(&[1], &[]);
    assert_wire_types::<v1::CredentialDestroyFailed>(&[1, 2], &[]);
    assert_wire_types::<v1::CredentialDestroyRequested>(&[1, 2], &[]);
    assert_wire_types::<v1::CredentialDestroyed>(&[1], &[]);
    assert_wire_types::<v1::CredentialRevoked>(&[1], &[]);
    assert_wire_types::<v1::CredentialRotated>(&[1, 2], &[]);
    assert_wire_types::<v1::CredentialRotationFailed>(&[1, 2], &[]);
    assert_wire_types::<v1::CredentialRotationRequested>(&[1], &[]);
    assert_wire_types::<v1::CredentialWriteFailed>(&[1, 2], &[]);
    assert_wire_types::<v1::CredentialWriteRequested>(&[1, 2, 5], &[3, 4]);
    assert_wire_types::<v1::CredentialEvent>(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], &[]);
    assert_wire_types::<state_v1::CredentialMetadata>(&[1, 4], &[2, 3]);
    assert_wire_types::<state_v1::CredentialStateSnapshot>(&[1, 2, 3, 4, 5, 6, 7, 8, 9], &[]);
    assert_wire_types::<state_v1::PendingCredentialWriteState>(&[1, 2, 5], &[3, 4]);
    assert_wire_types::<state_v1::ActiveCredentialState>(&[1, 2], &[]);
    assert_wire_types::<state_v1::FailedCredentialWriteState>(&[1, 2], &[]);
    assert_wire_types::<state_v1::RotationPendingCredentialState>(&[1], &[]);
    assert_wire_types::<state_v1::RevokedCredentialState>(&[1], &[]);
    assert_wire_types::<state_v1::DestroyRequestedCredentialState>(&[1, 2], &[]);
    assert_wire_types::<state_v1::DestroyedCredentialState>(&[1], &[]);
    assert_wire_types::<state_v1::CleanupFailedCredentialState>(&[1, 2], &[]);
    assert_wire_types::<checkpoints_v1::RuntimeProjectionCheckpoint>(&[], &[1]);
    assert_wire_types::<checkpoints_v1::CredentialRecoveryWorkerCheckpoint>(&[], &[1, 2, 3, 4]);
    assert_wire_types::<idempotency_v1::CredentialManagementIdempotencyRecord>(&[1, 3], &[2]);
    assert_wire_types::<idempotency_v1::CredentialCommandResponse>(&[1, 3], &[2]);
    assert_wire_types::<idempotency_v1::CredentialRefResponse>(&[1, 3, 4, 5, 6, 7], &[2]);
}

#[test]
fn string_fields_reject_invalid_utf8() {
    assert_invalid_utf8::<v1::CredentialRef>(&[1, 3, 5]);
    assert_invalid_utf8::<v1::CredentialMetadata>(&[4]);
    assert_invalid_utf8::<v1::CredentialDestroyFailed>(&[2]);
    assert_invalid_utf8::<v1::CredentialDestroyRequested>(&[2]);
    assert_invalid_utf8::<v1::CredentialRotationFailed>(&[2]);
    assert_invalid_utf8::<v1::CredentialWriteFailed>(&[1, 2]);
    assert_invalid_utf8::<v1::CredentialWriteRequested>(&[1, 2, 5]);
    assert_invalid_utf8::<state_v1::CredentialMetadata>(&[4]);
    assert_invalid_utf8::<state_v1::PendingCredentialWriteState>(&[1, 2, 5]);
    assert_invalid_utf8::<state_v1::FailedCredentialWriteState>(&[1, 2]);
    assert_invalid_utf8::<state_v1::DestroyRequestedCredentialState>(&[2]);
    assert_invalid_utf8::<state_v1::CleanupFailedCredentialState>(&[2]);
    assert_invalid_utf8::<idempotency_v1::CredentialManagementIdempotencyRecord>(&[1]);
    assert_invalid_utf8::<idempotency_v1::CredentialCommandResponse>(&[1]);
    assert_invalid_utf8::<idempotency_v1::CredentialRefResponse>(&[1, 3, 4, 5, 6, 7]);
}

const BAD_LEAF: [u8; 3] = [0x0a, 1, 0xff];

fn bad_metadata() -> Vec<u8> {
    length_prefixed(1, &BAD_LEAF)
}

fn bad_reference_field() -> Vec<u8> {
    length_prefixed(1, &BAD_LEAF)
}

#[test]
fn nested_messages_propagate_malformed_leaf_wire() {
    assert_nested_invalid_utf8::<v1::CredentialMetadata>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialDestroyFailed>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialDestroyRequested>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialDestroyed>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialRevoked>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialRotationFailed>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialRotationRequested>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialRotated>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<v1::CredentialRotated>(&[2], &bad_metadata());
    assert_nested_invalid_utf8::<v1::CredentialActivated>(&[1], &bad_metadata());
    assert_nested_invalid_utf8::<state_v1::CredentialMetadata>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<state_v1::ActiveCredentialState>(&[1], &bad_metadata());
    assert_nested_invalid_utf8::<state_v1::ActiveCredentialState>(&[2], &BAD_LEAF);
    assert_nested_invalid_utf8::<state_v1::RotationPendingCredentialState>(&[1], &length_prefixed(1, &bad_metadata()));
    assert_nested_invalid_utf8::<state_v1::RevokedCredentialState>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<state_v1::DestroyRequestedCredentialState>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<state_v1::DestroyedCredentialState>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<state_v1::CleanupFailedCredentialState>(&[1], &BAD_LEAF);
    assert_nested_invalid_utf8::<idempotency_v1::CredentialCommandResponse>(&[3], &BAD_LEAF);
    assert_nested_invalid_utf8::<idempotency_v1::CredentialManagementIdempotencyRecord>(&[3], &BAD_LEAF);
    let mut event_payloads = vec![
        BAD_LEAF.to_vec(),
        BAD_LEAF.to_vec(),
        length_prefixed(1, &bad_metadata()),
    ];
    event_payloads.extend((0..7).map(|_| bad_reference_field()));
    for (index, payload) in event_payloads.iter().enumerate() {
        let field = u8::try_from(index + 1).expect("field");
        assert_malformed::<v1::CredentialEvent>(&length_prefixed(field, payload), DecodeError::InvalidUtf8);
        let valid: v1::CredentialEvent = serde_json::from_value(events()[index].clone()).expect("event");
        let repeated = [valid.encode_to_vec(), length_prefixed(field, payload)].concat();
        assert_malformed::<v1::CredentialEvent>(&repeated, DecodeError::InvalidUtf8);
    }
    let mut snapshot_payloads = vec![
        Vec::new(),
        BAD_LEAF.to_vec(),
        length_prefixed(1, &bad_metadata()),
        BAD_LEAF.to_vec(),
        length_prefixed(1, &length_prefixed(1, &bad_metadata())),
    ];
    snapshot_payloads.extend((0..4).map(|_| bad_reference_field()));
    for (index, payload) in snapshot_payloads.iter().enumerate().skip(1) {
        let field = u8::try_from(index + 1).expect("field");
        assert_malformed::<state_v1::CredentialStateSnapshot>(
            &length_prefixed(field, payload),
            DecodeError::InvalidUtf8,
        );
        let valid: state_v1::CredentialStateSnapshot =
            serde_json::from_value(snapshots()[index].clone()).expect("snapshot");
        let repeated = [valid.encode_to_vec(), length_prefixed(field, payload)].concat();
        assert_malformed::<state_v1::CredentialStateSnapshot>(&repeated, DecodeError::InvalidUtf8);
    }
    let invalid_tag = DecodeError::InvalidWireType(7);
    assert_malformed::<state_v1::CredentialMissingState>(&[0x07], invalid_tag.clone());
    assert_malformed::<state_v1::CredentialStateSnapshot>(&length_prefixed(1, &[0x07]), invalid_tag.clone());
    let missing: state_v1::CredentialStateSnapshot =
        serde_json::from_value(snapshots()[0].clone()).expect("missing state");
    let repeated = [missing.encode_to_vec(), length_prefixed(1, &[0x07])].concat();
    assert_malformed::<state_v1::CredentialStateSnapshot>(&repeated, invalid_tag);
    let response: idempotency_v1::CredentialCommandResponse =
        serde_json::from_value(command_response()).expect("response");
    let repeated = [response.encode_to_vec(), length_prefixed(3, &BAD_LEAF)].concat();
    assert_malformed::<idempotency_v1::CredentialCommandResponse>(&repeated, DecodeError::InvalidUtf8);
    let record = idempotency_v1::CredentialManagementIdempotencyRecord {
        fingerprint: "fp".into(),
        response: response.clone().into(),
        ..Default::default()
    };
    let repeated = [record.encode_to_vec(), length_prefixed(3, &BAD_LEAF)].concat();
    assert_malformed::<idempotency_v1::CredentialManagementIdempotencyRecord>(&repeated, DecodeError::InvalidUtf8);
    let doubled = [record.encode_to_vec(), record.encode_to_vec()].concat();
    let merged = idempotency_v1::CredentialManagementIdempotencyRecord::decode_from_slice(&doubled).expect("merged");
    assert_wire_codec(&doubled, &merged);
    let doubled = [response.encode_to_vec(), response.encode_to_vec()].concat();
    let merged = idempotency_v1::CredentialCommandResponse::decode_from_slice(&doubled).expect("merged");
    assert_wire_codec(&doubled, &merged);
}

#[test]
fn event_oneof_members_merge_when_a_variant_repeats() {
    for event in events() {
        let single: v1::CredentialEvent = serde_json::from_value(event).expect("event");
        let wire = single.encode_to_vec();
        let doubled = [wire.as_slice(), wire.as_slice()].concat();
        let expected = v1::CredentialEvent::decode_from_slice(&doubled).expect("merged event");
        assert_eq!(expected.event, single.event);
        assert_wire_codec(&doubled, &expected);
    }
}

#[test]
fn snapshot_oneof_members_merge_when_a_variant_repeats() {
    for snapshot in snapshots() {
        let single: state_v1::CredentialStateSnapshot = serde_json::from_value(snapshot).expect("snapshot");
        let wire = single.encode_to_vec();
        let doubled = [wire.as_slice(), wire.as_slice()].concat();
        let expected = state_v1::CredentialStateSnapshot::decode_from_slice(&doubled).expect("merged snapshot");
        assert_wire_codec(&doubled, &expected);
    }
    let active: state_v1::ActiveCredentialState = serde_json::from_value(json!({
        "metadata": {"reference": credential_ref(), "fingerprint": "a"},
        "previousVersions": [credential_ref()]
    }))
    .expect("active");
    let wire = [active.encode_to_vec(), active.encode_to_vec()].concat();
    let merged = state_v1::ActiveCredentialState::decode_from_slice(&wire).expect("merged active");
    assert_eq!(merged.previous_versions.len(), 2);
    assert_wire_codec(&wire, &merged);
}

fn entry(value: &Value) -> String {
    let text = value.to_string();
    text[1..text.len() - 1].to_owned()
}

fn assert_oneof_conflicts<M: DeserializeOwned + Serialize + Debug>(variants: &[Value]) {
    for (index, variant) in variants.iter().enumerate() {
        let next = &variants[(index + 1) % variants.len()];
        assert_rejected::<M>(&format!("{{{},{}}}", entry(variant), entry(next)));
        assert_rejected::<M>(&format!("{{{},{}}}", entry(next), entry(variant)));
        assert_rejected::<M>(&format!("{{{},{}}}", entry(variant), entry(variant)));
        assert_rejected::<M>(&variant.to_string().replace('{', "[").replace('}', "]"));
        assert_normalized::<M>(json!({"futureField": [1, {"a": null}]}), json!({}));
    }
    for variant in variants {
        let key = variant
            .as_object()
            .expect("variant")
            .keys()
            .next()
            .expect("key")
            .clone();
        assert_rejected::<M>(&format!("{{\"{key}\":true}}"));
    }
    for rejected in ["true", "1", "[]", "\"event\""] {
        assert_rejected::<M>(rejected);
    }
}

#[test]
fn oneof_json_rejects_conflicting_duplicate_and_malformed_members() {
    assert_oneof_conflicts::<v1::CredentialEvent>(&events());
    assert_oneof_conflicts::<state_v1::CredentialStateSnapshot>(&snapshots());
    let nulls = json!({
        "writeRequested": null, "writeFailed": null, "activated": null, "rotationRequested": null,
        "rotationFailed": null, "rotated": null, "revoked": null, "destroyRequested": null,
        "destroyed": null, "destroyFailed": null
    });
    assert_normalized::<v1::CredentialEvent>(nulls, json!({}));
    let nulls = json!({
        "missing": null, "pendingWrite": null, "active": null, "writeFailed": null,
        "rotationPending": null, "revoked": null, "destroyRequested": null, "destroyed": null,
        "cleanupFailed": null
    });
    assert_normalized::<state_v1::CredentialStateSnapshot>(nulls, json!({}));
    assert_normalized::<v1::CredentialEvent>(
        json!({"write_requested": {"credential_id": "c", "owner_id": "o", "scope_key": "s"}}),
        json!({"writeRequested": {"credentialId": "c", "ownerId": "o", "scopeKey": "s"}}),
    );
    assert_normalized::<state_v1::CredentialStateSnapshot>(
        json!({"pending_write": {"credential_id": "c", "owner_id": "o", "scope_key": "s"}}),
        json!({"pendingWrite": {"credentialId": "c", "ownerId": "o", "scopeKey": "s"}}),
    );
}

macro_rules! assert_case {
    ($case:ident, $payload:ty, $variant:ident) => {{
        let case = $case::from(<$payload>::default());
        assert!(matches!(case, $case::$variant(_)));
        let optional: Option<$case> = <$payload>::default().into();
        assert!(matches!(optional, Some($case::$variant(_))));
    }};
}

#[test]
fn oneof_cases_convert_from_every_payload_type() {
    assert_case!(CredentialEventCase, v1::CredentialWriteRequested, WriteRequested);
    assert_case!(CredentialEventCase, v1::CredentialWriteFailed, WriteFailed);
    assert_case!(CredentialEventCase, v1::CredentialActivated, Activated);
    assert_case!(CredentialEventCase, v1::CredentialRotationRequested, RotationRequested);
    assert_case!(CredentialEventCase, v1::CredentialRotationFailed, RotationFailed);
    assert_case!(CredentialEventCase, v1::CredentialRotated, Rotated);
    assert_case!(CredentialEventCase, v1::CredentialRevoked, Revoked);
    assert_case!(CredentialEventCase, v1::CredentialDestroyRequested, DestroyRequested);
    assert_case!(CredentialEventCase, v1::CredentialDestroyed, Destroyed);
    assert_case!(CredentialEventCase, v1::CredentialDestroyFailed, DestroyFailed);
    assert_case!(CredentialStateSnapshotCase, state_v1::CredentialMissingState, Missing);
    assert_case!(
        CredentialStateSnapshotCase,
        state_v1::PendingCredentialWriteState,
        PendingWrite
    );
    assert_case!(CredentialStateSnapshotCase, state_v1::ActiveCredentialState, Active);
    assert_case!(
        CredentialStateSnapshotCase,
        state_v1::FailedCredentialWriteState,
        WriteFailed
    );
    assert_case!(
        CredentialStateSnapshotCase,
        state_v1::RotationPendingCredentialState,
        RotationPending
    );
    assert_case!(CredentialStateSnapshotCase, state_v1::RevokedCredentialState, Revoked);
    assert_case!(
        CredentialStateSnapshotCase,
        state_v1::DestroyRequestedCredentialState,
        DestroyRequested
    );
    assert_case!(
        CredentialStateSnapshotCase,
        state_v1::DestroyedCredentialState,
        Destroyed
    );
    assert_case!(
        CredentialStateSnapshotCase,
        state_v1::CleanupFailedCredentialState,
        CleanupFailed
    );
}

struct FailingWriter {
    remaining: usize,
}

impl std::io::Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("sink closed"));
        }
        let written = buf.len().min(self.remaining);
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn assert_write_failures<T: Serialize>(value: &T) {
    let complete = serde_json::to_vec(value).expect("complete JSON");
    for capacity in 0..complete.len() {
        assert!(
            serde_json::to_writer(FailingWriter { remaining: capacity }, value).is_err(),
            "capacity {capacity}"
        );
    }
}

fn assert_serialization_failures<M>(json: Value)
where
    M: Message + HasMessageView + DeserializeOwned + Serialize,
    for<'a> M::View<'a>: Serialize,
    M::ViewHandle: Serialize,
{
    let message: M = serde_json::from_value(json).expect("fixture");
    assert_write_failures(&message);
    let wire = message.encode_to_vec();
    let view = M::decode_view(&wire).expect("view");
    assert_write_failures(&view);
    drop(view);
    let handle = M::decode_view_handle(buffa::bytes::Bytes::from(wire)).expect("handle");
    assert_write_failures(&handle);
}

#[test]
fn json_serialization_surfaces_writer_failures_for_every_message() {
    let events = events();
    let snapshots = snapshots();
    let payload = |value: &Value| {
        value
            .as_object()
            .expect("oneof")
            .values()
            .next()
            .expect("payload")
            .clone()
    };
    assert_serialization_failures::<v1::CredentialRef>(credential_ref());
    assert_serialization_failures::<v1::CredentialMetadata>(metadata());
    assert_serialization_failures::<state_v1::CredentialMetadata>(state_metadata());
    for event in &events {
        assert_serialization_failures::<v1::CredentialEvent>(event.clone());
    }
    assert_serialization_failures::<v1::CredentialWriteRequested>(payload(&events[0]));
    assert_serialization_failures::<v1::CredentialWriteFailed>(payload(&events[1]));
    assert_serialization_failures::<v1::CredentialActivated>(payload(&events[2]));
    assert_serialization_failures::<v1::CredentialRotationRequested>(payload(&events[3]));
    assert_serialization_failures::<v1::CredentialRotationFailed>(payload(&events[4]));
    assert_serialization_failures::<v1::CredentialRotated>(payload(&events[5]));
    assert_serialization_failures::<v1::CredentialRevoked>(payload(&events[6]));
    assert_serialization_failures::<v1::CredentialDestroyRequested>(payload(&events[7]));
    assert_serialization_failures::<v1::CredentialDestroyed>(payload(&events[8]));
    assert_serialization_failures::<v1::CredentialDestroyFailed>(payload(&events[9]));
    for snapshot in &snapshots {
        assert_serialization_failures::<state_v1::CredentialStateSnapshot>(snapshot.clone());
    }
    assert_serialization_failures::<state_v1::CredentialMissingState>(payload(&snapshots[0]));
    assert_serialization_failures::<state_v1::PendingCredentialWriteState>(payload(&snapshots[1]));
    assert_serialization_failures::<state_v1::ActiveCredentialState>(payload(&snapshots[2]));
    assert_serialization_failures::<state_v1::FailedCredentialWriteState>(payload(&snapshots[3]));
    assert_serialization_failures::<state_v1::RotationPendingCredentialState>(payload(&snapshots[4]));
    assert_serialization_failures::<state_v1::RevokedCredentialState>(payload(&snapshots[5]));
    assert_serialization_failures::<state_v1::DestroyRequestedCredentialState>(payload(&snapshots[6]));
    assert_serialization_failures::<state_v1::DestroyedCredentialState>(payload(&snapshots[7]));
    assert_serialization_failures::<state_v1::CleanupFailedCredentialState>(payload(&snapshots[8]));
    assert_serialization_failures::<checkpoints_v1::RuntimeProjectionCheckpoint>(json!({"lastScannedSequence": "5"}));
    assert_serialization_failures::<checkpoints_v1::CredentialRecoveryWorkerCheckpoint>(json!({
        "lastScannedSequence": "1", "consecutiveFailureCount": 2,
        "firstFailureUnixSeconds": "3", "retryAfterUnixSeconds": "4"
    }));
    assert_serialization_failures::<idempotency_v1::CredentialManagementIdempotencyRecord>(json!({
        "fingerprint": "fp", "status": "CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED",
        "response": command_response()
    }));
    assert_serialization_failures::<idempotency_v1::CredentialCommandResponse>(command_response());
    assert_serialization_failures::<idempotency_v1::CredentialRefResponse>(command_response()["credentialRef"].clone());
}

#[test]
fn previous_version_collections_respect_element_memory_limits() {
    let active: state_v1::ActiveCredentialState = serde_json::from_value(json!({
        "metadata": state_metadata(), "previousVersions": [credential_ref()]
    }))
    .expect("active");
    super::assert_collection_limit::<state_v1::ActiveCredentialState>(&active.encode_to_vec());
}
