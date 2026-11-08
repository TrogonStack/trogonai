use std::fmt::Debug;

use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, Enumeration, HasMessageView, Message};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::{assert_json_codec, assert_malformed, assert_wire_codec, fixtures};
use crate::peers::{state_v1alpha1 as state, v1alpha1 as proto};

fn field_contract<M: Message + HasMessageView>(length_delimited: &[u8], varints: &[u8], nested: &[u8]) {
    assert_malformed::<M>(&[0], DecodeError::InvalidFieldNumber);
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
            assert_malformed::<M>(
                &[field << 3 | expected, if expected == 2 { 1 } else { 0x80 }],
                DecodeError::UnexpectedEof,
            );
        }
    }
    for &field in nested {
        assert_malformed::<M>(&[field << 3 | 2, 1, 0], DecodeError::InvalidFieldNumber);
        assert_malformed::<M>(
            &[field << 3 | 2, 0, field << 3 | 2, 1, 0],
            DecodeError::InvalidFieldNumber,
        );
    }
}

#[test]
fn peer_metadata_and_state_fields_reject_wrong_wire_types_and_truncated_nested_values() {
    field_contract::<proto::AttachmentRef>(&[1, 2], &[3], &[]);
    field_contract::<proto::PeerEnrollment>(&[1, 2, 3, 4, 5, 6, 7, 8], &[], &[8]);
    field_contract::<proto::PeerAttachment>(&[1, 3, 4], &[2], &[1, 3, 4]);
    field_contract::<proto::PeerPresence>(&[1, 2, 6, 7, 8], &[3, 4, 5], &[6, 7, 8]);
    field_contract::<proto::MessageMetadata>(&[1, 2, 3, 4, 8, 9, 10, 11, 12], &[6], &[8, 9, 11, 12]);
    field_contract::<proto::ContentSemanticFingerprint>(&[1], &[], &[]);
    field_contract::<proto::StorageLocator>(&[1], &[], &[]);
    field_contract::<proto::Digest>(&[1, 2], &[], &[]);
    field_contract::<proto::ClaimCheck>(&[1, 2], &[], &[1, 2]);
    field_contract::<proto::MessageContent>(&[1], &[], &[]);
    field_contract::<proto::RecipientReceipt>(&[1, 2, 4, 6], &[3, 5], &[6]);
    field_contract::<proto::OutboxEntry>(&[1, 2, 3, 4], &[], &[3, 4]);
    field_contract::<proto::InboxEntry>(&[1, 3], &[2], &[1, 3]);
    field_contract::<proto::PeerBatch>(&[1, 2, 3, 4], &[], &[3, 4]);
    field_contract::<proto::PreparedMessage>(&[1, 2], &[], &[1]);
    field_contract::<state::PeerState>(&[1, 2, 5, 6, 7, 8, 9], &[3, 4, 10], &[1, 2, 5, 6, 7, 8, 9]);
}

#[test]
fn peer_event_payloads_validate_every_registered_embedded_fact() {
    field_contract::<proto::PeerEnrolled>(&[1], &[], &[1]);
    field_contract::<proto::PeerAttached>(&[1], &[], &[1]);
    field_contract::<proto::PeerHeartbeatRecorded>(&[1, 4], &[2, 3], &[1, 4]);
    field_contract::<proto::PeerDetached>(&[1, 2], &[], &[1, 2]);
    field_contract::<proto::PeerMessageAccepted>(&[1], &[], &[1]);
    field_contract::<proto::PeerMessageIntakeRecorded>(&[1], &[], &[1]);
    field_contract::<proto::PeerBatchPrepared>(&[1, 2], &[], &[1, 2]);
    field_contract::<proto::PeerBatchAcknowledged>(&[1, 2], &[], &[2]);
    field_contract::<proto::PeerMessageDisposed>(&[1], &[], &[1]);
    field_contract::<proto::PeerMessageSettled>(&[1], &[], &[1]);
    field_contract::<proto::PeerEvent>(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], &[], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
}

#[test]
fn peer_rpc_inputs_and_replies_share_strict_field_and_nested_decoding() {
    field_contract::<proto::EnrollRequest>(&[1, 2, 3, 4, 5], &[], &[]);
    field_contract::<proto::EnrollResponse>(&[1, 2], &[], &[2]);
    field_contract::<proto::HeartbeatRequest>(&[1, 4], &[2, 3], &[1, 4]);
    field_contract::<proto::HeartbeatResponse>(&[1], &[], &[1]);
    field_contract::<proto::DetachRequest>(&[1], &[], &[1]);
    field_contract::<proto::DetachResponse>(&[], &[], &[]);
    field_contract::<proto::RosterRequest>(&[1, 3], &[2], &[1]);
    field_contract::<proto::RosterResponse>(&[1, 2], &[], &[1]);
    field_contract::<proto::SendRequest>(&[1, 2, 3, 4, 5], &[], &[1]);
    field_contract::<proto::SendResponse>(&[1], &[], &[1]);
    field_contract::<proto::StatusRequest>(&[1, 2], &[], &[1]);
    field_contract::<proto::StatusResponse>(&[1, 3], &[2], &[1, 3]);
    field_contract::<proto::PrepareBatchRequest>(&[1, 2], &[], &[1]);
    field_contract::<proto::PrepareBatchResponse>(&[1, 2], &[3], &[1, 2]);
    field_contract::<proto::AcknowledgeBatchRequest>(&[1, 2], &[], &[1]);
    field_contract::<proto::AcknowledgeBatchResponse>(&[1], &[], &[1]);
}

fn collection_limit<M: Message + HasMessageView>(wire: &[u8]) {
    let forbidden = DecodeOptions::new().with_element_memory_limit(0);
    assert_eq!(
        forbidden.decode_from_slice::<M>(wire).err(),
        Some(DecodeError::ElementMemoryLimitExceeded)
    );
    assert_eq!(
        M::decode_view_with_options(wire, &forbidden).err(),
        Some(DecodeError::ElementMemoryLimitExceeded)
    );
    assert_eq!(
        M::decode_view_handle_with_options(Bytes::copy_from_slice(wire), &forbidden).err(),
        Some(DecodeError::ElementMemoryLimitExceeded)
    );
    let allowed = DecodeOptions::new().with_element_memory_limit(4096);
    assert!(allowed.decode_from_slice::<M>(wire).is_ok());
    assert!(M::decode_view_with_options(wire, &allowed).is_ok());
}

#[test]
fn mailbox_and_roster_collections_charge_even_empty_elements_against_decode_limits() {
    collection_limit::<state::PeerState>(b"\x32\x00");
    collection_limit::<state::PeerState>(b"\x3a\x00");
    collection_limit::<state::PeerState>(b"\x42\x00");
    collection_limit::<state::PeerState>(b"\x4a\x00");
    collection_limit::<proto::PeerBatch>(b"\x12\x00");
    collection_limit::<proto::PeerBatchPrepared>(b"\x12\x00");
    collection_limit::<proto::RosterResponse>(b"\x0a\x00");
    collection_limit::<proto::PrepareBatchResponse>(b"\x12\x00");
}

#[test]
fn attachment_fragments_merge_while_later_scalar_values_replace_earlier_values() {
    let first = assert_json_codec::<proto::SendRequest>(json!({
        "attachmentRef": {"peerId": "peer_alpha", "connectorId": "connector_old"},
        "recipientPeerId": "peer_beta", "idempotencyKey": "send_original", "text": "first"
    }));
    let second = assert_json_codec::<proto::SendRequest>(json!({
        "attachmentRef": {"connectorId": "connector_alpha", "generation": "7"},
        "idempotencyKey": "send_current", "text": "second"
    }));
    let expected = assert_json_codec::<proto::SendRequest>(json!({
        "attachmentRef": fixtures::attachment_ref(), "recipientPeerId": "peer_beta",
        "idempotencyKey": "send_current", "text": "second"
    }));
    assert_wire_codec(&[first.encode_to_vec(), second.encode_to_vec()].concat(), &expected);
}

#[test]
fn peer_state_wire_fragments_append_intake_in_order_and_keep_the_latest_counter() {
    let first = assert_json_codec::<state::PeerState>(
        json!({"inbox": [fixtures::inbox("message_incoming_one", 7)], "nextIntakeOrdinal": "8"}),
    );
    let second = assert_json_codec::<state::PeerState>(
        json!({"inbox": [fixtures::inbox("message_incoming_two", 8)], "nextIntakeOrdinal": "9"}),
    );
    let expected = assert_json_codec::<state::PeerState>(json!({
        "inbox": [fixtures::inbox("message_incoming_one", 7), fixtures::inbox("message_incoming_two", 8)], "nextIntakeOrdinal": "9"
    }));
    assert_wire_codec(&[first.encode_to_vec(), second.encode_to_vec()].concat(), &expected);
}

#[test]
fn utf8_text_size_and_wire_message_limits_are_distinct() {
    let text = "é".repeat(1024);
    assert_eq!(text.len(), 2048);
    let message = assert_json_codec::<proto::MessageContent>(json!({"text": text}));
    let wire = message.encode_to_vec();
    assert!(wire.len() > 2048);
    let body_sized_limit = DecodeOptions::new().with_max_message_size(2048);
    assert_eq!(
        body_sized_limit.decode_from_slice::<proto::MessageContent>(&wire).err(),
        Some(DecodeError::MessageTooLarge)
    );
    assert_malformed::<proto::MessageContent>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<proto::MessageMetadata>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<proto::SendRequest>(b"\x22\x01\xff", DecodeError::InvalidUtf8);
    assert_json_codec::<proto::MessageMetadata>(json!({"contentRef": {"digest": {"value": "/w=="}}}));
}

#[test]
fn json_identity_aliases_and_unsigned_boundaries_do_not_silently_change_values() {
    let alias: proto::AttachmentRef =
        serde_json::from_value(json!({"peer_id": "peer_alpha", "connector_id": "connector_alpha", "generation": 7}))
            .unwrap();
    assert_eq!(serde_json::to_value(alias).unwrap(), fixtures::attachment_ref());
    assert_json_codec::<proto::AttachmentRef>(json!({"generation": u64::MAX.to_string()}));
    assert_json_codec::<proto::RecipientReceipt>(json!({"recipientRevision": u64::MAX.to_string()}));
    assert_json_codec::<proto::InboxEntry>(json!({"intakeOrdinal": u64::MAX.to_string()}));
    assert_json_codec::<proto::MessageMetadata>(json!({"contentSizeBytes": u32::MAX}));
    let nulls: proto::AttachmentRef = serde_json::from_value(
        json!({"peerId": null, "connectorId": null, "generation": null, "future": {"nested": true}}),
    )
    .unwrap();
    assert_eq!(nulls, proto::AttachmentRef::default());
    for invalid in [
        r#"{"generation":"18446744073709551616"}"#,
        r#"{"generation":-1}"#,
        r#"{"peerId":"first","peer_id":"second"}"#,
        r#"{"generation":"1","generation":2}"#,
    ] {
        assert!(serde_json::from_str::<proto::AttachmentRef>(invalid).is_err());
    }
    for invalid in [
        json!({"contentSizeBytes": -1}),
        json!({"contentSizeBytes": 4_294_967_296_u64}),
        json!({"contentRef": {"digest": {"value": "!"}}}),
    ] {
        assert!(serde_json::from_value::<proto::MessageMetadata>(invalid).is_err());
    }
    assert!(serde_json::from_value::<proto::PeerBatch>(json!({"messageIds": [null]})).is_err());
    assert!(serde_json::from_value::<proto::PrepareBatchResponse>(json!({"messages": [null]})).is_err());
}

fn enum_contract<E>(names: &[&str])
where
    E: Enumeration + Serialize + DeserializeOwned + Debug + PartialEq + Copy + Default + 'static,
{
    assert_eq!(E::values().len(), names.len());
    for (number, &name) in names.iter().enumerate() {
        let value: E = serde_json::from_value(json!(name)).unwrap();
        assert_eq!(value.to_i32(), number as i32);
        assert_eq!(value.proto_name(), name);
        assert_eq!(E::from_i32(number as i32), Some(value));
        assert_eq!(E::from_proto_name(name), Some(value));
        assert_eq!(serde_json::to_value(value).unwrap(), json!(name));
        assert_eq!(serde_json::from_value::<E>(json!(number)).unwrap(), value);
        let signed = serde::de::value::I64Deserializer::<serde::de::value::Error>::new(number as i64);
        assert_eq!(<E as serde::Deserialize<'_>>::deserialize(signed).unwrap(), value);
    }
    for outside_i32 in [i64::MIN, i64::MAX] {
        let signed = serde::de::value::I64Deserializer::<serde::de::value::Error>::new(outside_i32);
        assert!(<E as serde::Deserialize<'_>>::deserialize(signed).is_err());
    }
    assert_eq!(serde_json::from_value::<E>(json!(null)).unwrap(), E::default());
    assert!(serde_json::from_value::<E>(json!("FUTURE_VALUE")).is_err());
    for invalid in [json!(-1), json!(999), json!(u64::MAX), json!(true), json!([])] {
        assert!(serde_json::from_value::<E>(invalid).is_err());
    }
    assert_eq!(E::from_i32(999), None);
    assert_eq!(E::from_proto_name("FUTURE_VALUE"), None);
}

#[test]
fn peer_enum_numbers_and_json_names_are_stable() {
    enum_contract::<proto::AttachmentStatus>(&[
        "ATTACHMENT_STATUS_UNSPECIFIED",
        "ATTACHMENT_STATUS_ATTACHED",
        "ATTACHMENT_STATUS_DETACHED",
    ]);
    enum_contract::<proto::PeerAvailability>(&[
        "PEER_AVAILABILITY_UNSPECIFIED",
        "PEER_AVAILABILITY_READY",
        "PEER_AVAILABILITY_BUSY",
        "PEER_AVAILABILITY_BLOCKED",
        "PEER_AVAILABILITY_DRAINING",
    ]);
    enum_contract::<proto::PeerCondition>(&[
        "PEER_CONDITION_UNSPECIFIED",
        "PEER_CONDITION_RATE_LIMIT",
        "PEER_CONDITION_AUTHENTICATION",
        "PEER_CONDITION_BUDGET",
        "PEER_CONDITION_APPROVAL",
        "PEER_CONDITION_INPUT",
        "PEER_CONDITION_FAILED",
    ]);
    enum_contract::<proto::DeliveryCapability>(&[
        "DELIVERY_CAPABILITY_UNSPECIFIED",
        "DELIVERY_CAPABILITY_ON_USER_PROMPT",
    ]);
    enum_contract::<proto::PreparationOutcome>(&[
        "PREPARATION_OUTCOME_UNSPECIFIED",
        "PREPARATION_OUTCOME_BATCH_PREPARED",
        "PREPARATION_OUTCOME_EXPIRED",
        "PREPARATION_OUTCOME_UNAVAILABLE",
    ]);
    enum_contract::<proto::ContentAvailability>(&[
        "CONTENT_AVAILABILITY_UNSPECIFIED",
        "CONTENT_AVAILABILITY_AVAILABLE",
        "CONTENT_AVAILABILITY_RETRY_UNAVAILABLE",
    ]);
    enum_contract::<proto::MessageStatus>(&[
        "MESSAGE_STATUS_UNSPECIFIED",
        "MESSAGE_STATUS_ACCEPTED",
        "MESSAGE_STATUS_INTAKE_RECORDED",
        "MESSAGE_STATUS_BATCH_PREPARED",
        "MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED",
        "MESSAGE_STATUS_EXPIRED",
        "MESSAGE_STATUS_UNAVAILABLE",
        "MESSAGE_STATUS_UNRESOLVED",
    ]);
}

#[test]
fn open_wire_enum_values_survive_for_the_domain_boundary_to_accept_or_reject() {
    let attachment = assert_json_codec::<proto::PeerAttachment>(json!({"status": 999}));
    assert_eq!(attachment.status.unwrap().as_known(), None);
    let presence = assert_json_codec::<proto::PeerPresence>(
        json!({"availability": 999, "condition": 999, "deliveryCapability": 999}),
    );
    assert_eq!(presence.availability.unwrap().as_known(), None);
    assert_eq!(presence.condition.unwrap().as_known(), None);
    assert_eq!(presence.delivery_capability.unwrap().as_known(), None);
    let receipt = assert_json_codec::<proto::RecipientReceipt>(json!({"outcome": 999}));
    assert_eq!(receipt.outcome.unwrap().as_known(), None);
    let batch = assert_json_codec::<proto::PrepareBatchResponse>(json!({"contentAvailability": 999}));
    assert_eq!(batch.content_availability.unwrap().as_known(), None);
    let status = assert_json_codec::<proto::StatusResponse>(json!({"status": 999}));
    assert_eq!(status.status.unwrap().as_known(), None);
}
