use buffa::Message;
use serde_json::{Value, json};

use super::{assert_json_codec, assert_wire_codec, fixtures, retained_message};
use crate::peers::{PeerEventCase, v1alpha1 as proto};

fn payload(name: &str) -> Value {
    fixtures::events()
        .into_iter()
        .find_map(|event| event.get(name).cloned())
        .expect("named peer event fixture")
}

#[test]
fn enrollment_attachment_and_heartbeat_facts_keep_their_own_payloads() {
    retained_message!(
        proto::PeerEnrolled,
        proto::PeerEnrolledOwnedView,
        payload("peerEnrolled"),
        |handle| {
            assert_eq!(handle.enrollment().as_option().unwrap().peer_id, Some("peer_alpha"));
        }
    );
    retained_message!(
        proto::PeerAttached,
        proto::PeerAttachedOwnedView,
        payload("peerAttached"),
        |handle| {
            assert_eq!(
                handle.attachment().as_option().unwrap().attachment_ref.generation,
                Some(7)
            );
        }
    );
    retained_message!(
        proto::PeerHeartbeatRecorded,
        proto::PeerHeartbeatRecordedOwnedView,
        payload("peerHeartbeatRecorded"),
        |handle| {
            assert_eq!(
                handle.attachment().as_option().unwrap().attachment_ref.connector_id,
                Some("connector_alpha")
            );
            assert_eq!(
                handle.availability().and_then(|value| value.as_known()),
                Some(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
            );
            assert_eq!(
                handle.condition().and_then(|value| value.as_known()),
                Some(proto::PeerCondition::PEER_CONDITION_RATE_LIMIT)
            );
            assert!(handle.active_at().is_set());
        }
    );
    retained_message!(
        proto::PeerDetached,
        proto::PeerDetachedOwnedView,
        payload("peerDetached"),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert!(handle.detached_at().is_set());
        }
    );
}

#[test]
fn acceptance_intake_and_settlement_facts_preserve_distinct_ownership_evidence() {
    retained_message!(
        proto::PeerMessageAccepted,
        proto::PeerMessageAcceptedOwnedView,
        payload("peerMessageAccepted"),
        |handle| {
            assert_eq!(
                handle.entry().as_option().unwrap().idempotency_key,
                Some("send_outgoing")
            );
            assert_eq!(
                handle.entry().as_option().unwrap().message.sender_peer_id,
                Some("peer_alpha")
            );
        }
    );
    retained_message!(
        proto::PeerMessageIntakeRecorded,
        proto::PeerMessageIntakeRecordedOwnedView,
        payload("peerMessageIntakeRecorded"),
        |handle| {
            assert_eq!(handle.entry().as_option().unwrap().intake_ordinal, Some(7));
            assert_eq!(
                handle.entry().as_option().unwrap().message.recipient_peer_id,
                Some("peer_alpha")
            );
        }
    );
    retained_message!(
        proto::PeerMessageDisposed,
        proto::PeerMessageDisposedOwnedView,
        payload("peerMessageDisposed"),
        |handle| {
            assert_eq!(
                handle
                    .receipt()
                    .as_option()
                    .unwrap()
                    .outcome
                    .and_then(|value| value.as_known()),
                Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED)
            );
            assert_eq!(
                handle.receipt().as_option().unwrap().message_id,
                Some("message_expired")
            );
        }
    );
    retained_message!(
        proto::PeerMessageSettled,
        proto::PeerMessageSettledOwnedView,
        payload("peerMessageSettled"),
        |handle| {
            assert_eq!(
                handle.receipt().as_option().unwrap().recipient_peer_id,
                Some("peer_beta")
            );
            assert_eq!(
                handle.receipt().as_option().unwrap().recipient_revision,
                Some(9_007_199_254_740_993)
            );
        }
    );
}

#[test]
fn batch_preparation_fact_commits_ordered_receipts_without_fabricating_acknowledgment() {
    retained_message!(
        proto::PeerBatchPrepared,
        proto::PeerBatchPreparedOwnedView,
        payload("peerBatchPrepared"),
        |handle| {
            let batch = handle.batch().as_option().unwrap();
            assert_eq!(batch.batch_id, Some("batch_local"));
            assert!(!batch.acknowledged_at.is_set());
            assert_eq!(handle.receipts().len(), batch.message_ids.len());
            for (receipt, assigned) in handle.receipts().iter().zip(batch.message_ids.iter()) {
                assert_eq!(receipt.message_id, Some(*assigned));
                assert_eq!(receipt.batch_id, batch.batch_id);
            }
        }
    );
    retained_message!(
        proto::PeerBatchAcknowledged,
        proto::PeerBatchAcknowledgedOwnedView,
        payload("peerBatchAcknowledged"),
        |handle| {
            assert_eq!(handle.batch_id(), Some("batch_local"));
            assert!(handle.acknowledged_at().is_set());
        }
    );
}

#[test]
fn retained_peer_event_oneof_dispatch_covers_each_registered_fact() {
    for fixture in fixtures::events() {
        let expected = fixture.clone();
        retained_message!(proto::PeerEvent, proto::PeerEventOwnedView, fixture, |handle| {
            assert!(handle.event().is_some());
            assert_eq!(serde_json::to_value(handle.to_owned_message()).unwrap(), expected);
        });
    }
}

fn promoted_payload<Payload>(name: &str)
where
    Payload:
        Message + serde::de::DeserializeOwned + serde::Serialize + buffa::HasMessageView + std::fmt::Debug + PartialEq,
    for<'a> Payload::View<'a>: buffa::ViewEncode<'a> + serde::Serialize,
    Payload::ViewHandle: serde::Serialize,
    PeerEventCase: From<Payload>,
    Option<PeerEventCase>: From<Payload>,
{
    let fixture = payload(name);
    let inner = assert_json_codec::<Payload>(fixture.clone());
    let direct = PeerEventCase::from(inner.clone());
    let optional: Option<PeerEventCase> = inner.into();
    assert_eq!(optional.as_ref(), Some(&direct));
    let promoted = proto::PeerEvent { event: optional };
    let mut expected = serde_json::Map::new();
    expected.insert(name.to_owned(), fixture);
    assert_eq!(serde_json::to_value(&promoted).unwrap(), Value::Object(expected));
    assert_wire_codec(&promoted.encode_to_vec(), &promoted);
}

#[test]
fn public_payload_promotions_select_the_corresponding_wire_oneof_case() {
    promoted_payload::<proto::PeerEnrolled>("peerEnrolled");
    promoted_payload::<proto::PeerAttached>("peerAttached");
    promoted_payload::<proto::PeerHeartbeatRecorded>("peerHeartbeatRecorded");
    promoted_payload::<proto::PeerDetached>("peerDetached");
    promoted_payload::<proto::PeerMessageAccepted>("peerMessageAccepted");
    promoted_payload::<proto::PeerMessageIntakeRecorded>("peerMessageIntakeRecorded");
    promoted_payload::<proto::PeerBatchPrepared>("peerBatchPrepared");
    promoted_payload::<proto::PeerBatchAcknowledged>("peerBatchAcknowledged");
    promoted_payload::<proto::PeerMessageDisposed>("peerMessageDisposed");
    promoted_payload::<proto::PeerMessageSettled>("peerMessageSettled");
}

#[test]
fn same_event_case_fragments_merge_nested_batch_fields_and_append_receipts() {
    let first = assert_json_codec::<proto::PeerEvent>(json!({"peerBatchPrepared": {
        "batch": {"batchId": "batch_local", "messageIds": ["message_incoming_one"]},
        "receipts": [fixtures::incoming_receipt("message_incoming_one", 11)]
    }}));
    let second = assert_json_codec::<proto::PeerEvent>(json!({"peerBatchPrepared": {
        "batch": {"messageIds": ["message_incoming_two"], "preparedAt": fixtures::PREPARED},
        "receipts": [fixtures::incoming_receipt("message_incoming_two", 12)]
    }}));
    let expected = assert_json_codec::<proto::PeerEvent>(json!({"peerBatchPrepared": {
        "batch": fixtures::batch(), "receipts": [fixtures::incoming_receipt("message_incoming_one", 11), fixtures::incoming_receipt("message_incoming_two", 12)]
    }}));
    let wire = [first.encode_to_vec(), second.encode_to_vec()].concat();
    assert_wire_codec(&wire, &expected);
    let merged = proto::PeerEvent::decode_from_slice(&wire).unwrap();
    assert!(matches!(merged.event, Some(PeerEventCase::PeerBatchPrepared(ref fact)) if fact.receipts.len() == 2));
}

#[test]
fn a_different_oneof_case_replaces_the_previous_case_without_mixing_facts() {
    let enrolled =
        assert_json_codec::<proto::PeerEvent>(json!({"peerEnrolled": {"enrollment": fixtures::enrollment()}}));
    let detached = assert_json_codec::<proto::PeerEvent>(
        json!({"peerDetached": {"attachmentRef": fixtures::attachment_ref(), "detachedAt": fixtures::LEASE_END}}),
    );
    let wire = [enrolled.encode_to_vec(), detached.encode_to_vec()].concat();
    assert_wire_codec(&wire, &detached);
    let duplicate_oneof = json!({"peerEnrolled": {}, "peerDetached": {}});
    assert!(serde_json::from_value::<proto::PeerEvent>(duplicate_oneof).is_err());
}

#[test]
fn unknown_wire_fields_leave_the_event_oneof_absent() {
    let unknown = proto::PeerEvent::decode_from_slice(b"\xf8\x07\x01").unwrap();
    assert!(unknown.event.is_none());
    assert!(unknown.encode_to_vec().is_empty());
    let retained = proto::PeerEventOwnedView::decode(buffa::bytes::Bytes::from_static(b"\xf8\x07\x01")).unwrap();
    assert!(retained.event().is_none());
}
