use buffa::Message;
use serde_json::json;

use super::{assert_json_codec, fixtures, retained_message};
use crate::peers::{state_v1alpha1 as state, v1alpha1 as proto};

#[test]
fn identity_and_presence_views_keep_owner_attachment_and_activity_distinct() {
    retained_message!(
        proto::AttachmentRef,
        proto::AttachmentRefOwnedView,
        fixtures::attachment_ref(),
        |handle| {
            assert_eq!(handle.peer_id(), Some("peer_alpha"));
            assert_eq!(handle.connector_id(), Some("connector_alpha"));
            assert_eq!(handle.generation(), Some(7));
        }
    );
    retained_message!(
        proto::PeerEnrollment,
        proto::PeerEnrollmentOwnedView,
        fixtures::enrollment(),
        |handle| {
            assert_eq!(handle.peer_id(), Some("peer_alpha"));
            assert_eq!(handle.tenant_id(), Some("tenant_alpha"));
            assert_eq!(handle.caller_id(), Some("caller_alpha"));
            assert_eq!(handle.installation_id(), Some("installation_alpha"));
            assert_eq!(handle.native_session_id(), Some("native_alpha"));
            assert_eq!(handle.collaboration_ref(), Some("collaboration_alpha"));
            assert_eq!(handle.display_name(), Some("Review"));
            assert!(handle.enrolled_at().is_set());
        }
    );
    retained_message!(
        proto::PeerAttachment,
        proto::PeerAttachmentOwnedView,
        fixtures::attachment(),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().generation, Some(7));
            assert_eq!(
                handle.status().and_then(|status| status.as_known()),
                Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
            );
            let observed = handle.observed_at().as_option().unwrap();
            let expires = handle.expires_at().as_option().unwrap();
            assert_eq!(expires.seconds - observed.seconds, 10);
        }
    );
    retained_message!(
        proto::PeerPresence,
        proto::PeerPresenceOwnedView,
        fixtures::presence(),
        |handle| {
            assert_eq!(handle.peer_id(), Some("peer_alpha"));
            assert_eq!(handle.display_name(), Some("Review"));
            assert_eq!(
                handle.availability().and_then(|value| value.as_known()),
                Some(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
            );
            assert_eq!(
                handle.condition().and_then(|value| value.as_known()),
                Some(proto::PeerCondition::PEER_CONDITION_RATE_LIMIT)
            );
            assert_eq!(
                handle.delivery_capability().and_then(|value| value.as_known()),
                Some(proto::DeliveryCapability::DELIVERY_CAPABILITY_ON_USER_PROMPT)
            );
            assert!(
                handle.active_at().as_option().unwrap().seconds < handle.observed_at().as_option().unwrap().seconds
            );
            assert!(
                handle.expires_at().as_option().unwrap().seconds > handle.observed_at().as_option().unwrap().seconds
            );
        }
    );
}

#[test]
fn message_views_keep_durable_metadata_separate_from_erasable_text() {
    retained_message!(
        proto::MessageMetadata,
        proto::MessageMetadataOwnedView,
        fixtures::metadata(),
        |handle| {
            assert_eq!(handle.message_id(), Some("message_outgoing"));
            assert_eq!(handle.sender_peer_id(), Some("peer_alpha"));
            assert_eq!(handle.recipient_peer_id(), Some("peer_beta"));
            assert_eq!(handle.collaboration_ref(), Some("collaboration_alpha"));
            let metadata = handle.to_owned_message();
            let reference = metadata.content_ref.as_option().unwrap();
            assert_eq!(reference.storage_ref.value.as_deref(), Some("content_outgoing"));
            assert_eq!(reference.digest.algorithm.as_deref(), Some("sha256"));
            assert_eq!(reference.digest.value.as_deref(), Some(&[0; 32][..]));
            let semantic = metadata.semantic_fingerprint.as_option().unwrap();
            assert_eq!(semantic.value.as_deref(), Some(&[1; 32][..]));
            assert_ne!(reference.digest.value, semantic.value);
            assert_eq!(handle.content_size_bytes(), Some(10));
            assert_eq!(handle.reply_to_message_id(), Some("message_previous"));
            assert_eq!(
                handle.expires_at().as_option().unwrap().seconds - handle.accepted_at().as_option().unwrap().seconds,
                3_600
            );
        }
    );
    retained_message!(
        proto::MessageContent,
        proto::MessageContentOwnedView,
        json!({"text": "hello peer"}),
        |handle| {
            assert_eq!(handle.text(), Some("hello peer"));
            assert_eq!(handle.text().unwrap().len(), 10);
        }
    );
    retained_message!(
        proto::PreparedMessage,
        proto::PreparedMessageOwnedView,
        fixtures::prepared_message("message_incoming_one"),
        |handle| {
            assert_eq!(
                handle.metadata().as_option().unwrap().message_id,
                Some("message_incoming_one")
            );
            assert_eq!(
                handle.metadata().as_option().unwrap().recipient_peer_id,
                Some("peer_alpha")
            );
            assert_eq!(handle.text(), Some("hello peer"));
            assert_eq!(
                handle.metadata().as_option().unwrap().content_size_bytes,
                Some(handle.text().unwrap().len() as u32)
            );
        }
    );
}

#[test]
fn inbox_outbox_receipts_and_batches_preserve_retry_and_settlement_identity() {
    retained_message!(
        proto::RecipientReceipt,
        proto::RecipientReceiptOwnedView,
        fixtures::receipt(),
        |handle| {
            assert_eq!(handle.message_id(), Some("message_outgoing"));
            assert_eq!(handle.recipient_peer_id(), Some("peer_beta"));
            assert_eq!(
                handle.outcome().and_then(|value| value.as_known()),
                Some(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED)
            );
            assert_eq!(handle.batch_id(), Some("batch_remote"));
            assert_eq!(handle.recipient_revision(), Some(9_007_199_254_740_993));
            assert!(handle.decided_at().is_set());
        }
    );
    retained_message!(
        proto::OutboxEntry,
        proto::OutboxEntryOwnedView,
        fixtures::outbox(),
        |handle| {
            assert_eq!(handle.idempotency_key(), Some("send_outgoing"));
            assert_eq!(handle.semantic_digest(), Some(&[0; 32][..]));
            let message = handle.message().as_option().unwrap();
            let settled = handle.settlement().as_option().unwrap();
            assert_eq!(message.message_id, settled.message_id);
            assert_eq!(message.recipient_peer_id, settled.recipient_peer_id);
        }
    );
    retained_message!(
        proto::InboxEntry,
        proto::InboxEntryOwnedView,
        fixtures::inbox("message_incoming_one", 7),
        |handle| {
            assert_eq!(handle.intake_ordinal(), Some(7));
            let message = handle.message().as_option().unwrap();
            let disposition = handle.disposition().as_option().unwrap();
            assert_eq!(message.message_id, disposition.message_id);
            assert_eq!(message.recipient_peer_id, disposition.recipient_peer_id);
        }
    );
    retained_message!(
        proto::PeerBatch,
        proto::PeerBatchOwnedView,
        fixtures::acknowledged_batch(),
        |handle| {
            assert_eq!(handle.batch_id(), Some("batch_local"));
            assert_eq!(
                handle.message_ids().iter().copied().collect::<Vec<_>>(),
                ["message_incoming_one", "message_incoming_two"]
            );
            assert!(
                handle.prepared_at().as_option().unwrap().seconds
                    < handle.acknowledged_at().as_option().unwrap().seconds
            );
        }
    );
}

#[test]
fn peer_state_retention_keeps_mailbox_order_and_attachment_fence_together() {
    retained_message!(
        state::PeerState,
        state::PeerStateOwnedView,
        fixtures::state(),
        |handle| {
            assert_eq!(handle.enrollment().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert_eq!(
                handle.attachment().as_option().unwrap().attachment_ref.generation,
                Some(7)
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
            assert_eq!(handle.outbox().len(), 1);
            assert_eq!(handle.inbox().len(), 2);
            assert_eq!(handle.inbox()[0].intake_ordinal, Some(7));
            assert_eq!(handle.inbox()[1].intake_ordinal, Some(8));
            assert_eq!(handle.batches()[0].message_ids.len(), 2);
            assert_eq!(
                handle.dispositions()[0].outcome.and_then(|value| value.as_known()),
                Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED)
            );
            assert_eq!(handle.next_intake_ordinal(), Some(9));
        }
    );
}

#[test]
fn wire_presence_preserves_zero_empty_values_and_empty_nested_messages() {
    let absent = assert_json_codec::<proto::AttachmentRef>(json!({}));
    let present =
        assert_json_codec::<proto::AttachmentRef>(json!({"peerId": "", "connectorId": "", "generation": "0"}));
    assert_eq!(absent.generation, None);
    assert_eq!(present.generation, Some(0));
    assert_eq!(present.peer_id.as_deref(), Some(""));
    assert_ne!(absent, present);
    assert_json_codec::<proto::PeerAttachment>(json!({"attachmentRef": {}, "status": "ATTACHMENT_STATUS_UNSPECIFIED"}));
    assert_json_codec::<proto::MessageMetadata>(
        json!({"contentSizeBytes": 0, "contentRef": {"digest": {"value": ""}}}),
    );
    assert_json_codec::<proto::InboxEntry>(json!({"message": {}, "intakeOrdinal": "0", "disposition": {}}));
    assert_json_codec::<proto::OutboxEntry>(
        json!({"idempotencyKey": "", "semanticDigest": "", "message": {}, "settlement": {}}),
    );
    assert_json_codec::<proto::PeerBatch>(json!({"batchId": ""}));
    assert_json_codec::<state::PeerState>(json!({"enrollment": {}, "attachment": {}, "nextIntakeOrdinal": "0"}));
    for empty in [json!({"messageIds": []}), json!({"messageIds": null})] {
        let batch: proto::PeerBatch = serde_json::from_value(empty).unwrap();
        assert!(batch.message_ids.is_empty());
        assert_eq!(serde_json::to_value(batch).unwrap(), json!({}));
    }
    for empty in [
        json!({"inbox": [], "outbox": [], "batches": [], "dispositions": []}),
        json!({"inbox": null, "outbox": null, "batches": null, "dispositions": null}),
    ] {
        let state: state::PeerState = serde_json::from_value(empty).unwrap();
        assert!(
            state.inbox.is_empty()
                && state.outbox.is_empty()
                && state.batches.is_empty()
                && state.dispositions.is_empty()
        );
    }
}

#[test]
fn stored_content_reference_views_preserve_locator_and_digest_as_one_value() {
    retained_message!(
        proto::ContentSemanticFingerprint,
        proto::ContentSemanticFingerprintOwnedView,
        fixtures::semantic_fingerprint(),
        |handle| {
            assert_eq!(handle.value(), Some(&[1; 32][..]));
        }
    );
    retained_message!(
        proto::StorageLocator,
        proto::StorageLocatorOwnedView,
        json!({"value": "content_outgoing"}),
        |handle| {
            assert_eq!(handle.value(), Some("content_outgoing"));
        }
    );
    retained_message!(
        proto::Digest,
        proto::DigestOwnedView,
        json!({"algorithm": "sha256", "value": fixtures::DIGEST}),
        |handle| {
            assert_eq!(handle.algorithm(), Some("sha256"));
            assert_eq!(handle.value(), Some(&[0; 32][..]));
        }
    );
    retained_message!(
        proto::ClaimCheck,
        proto::ClaimCheckOwnedView,
        fixtures::content_ref(),
        |handle| {
            let reference = handle.to_owned_message();
            assert_eq!(reference.storage_ref.value.as_deref(), Some("content_outgoing"));
            assert_eq!(reference.digest.algorithm.as_deref(), Some("sha256"));
            assert_eq!(reference.digest.value.as_deref(), Some(&[0; 32][..]));
        }
    );
}
