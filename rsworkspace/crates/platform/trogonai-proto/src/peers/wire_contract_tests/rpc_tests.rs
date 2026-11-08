use buffa::Message;
use serde_json::json;

use super::{assert_json_codec, fixtures, retained_message};
use crate::peers::v1alpha1 as proto;

#[test]
fn enrollment_requests_keep_native_correlation_separate_from_service_identity() {
    let request = json!({
        "installationId": "installation_alpha", "nativeSessionId": "native_alpha",
        "connectorId": "connector_alpha", "collaborationRef": "collaboration_alpha", "displayName": "Review"
    });
    retained_message!(proto::EnrollRequest, proto::EnrollRequestOwnedView, request, |handle| {
        assert_eq!(handle.installation_id(), Some("installation_alpha"));
        assert_eq!(handle.native_session_id(), Some("native_alpha"));
        assert_eq!(handle.connector_id(), Some("connector_alpha"));
        assert_eq!(handle.collaboration_ref(), Some("collaboration_alpha"));
        assert_eq!(handle.display_name(), Some("Review"));
        let encoded = serde_json::to_value(handle.view()).unwrap();
        assert!(encoded.get("tenantId").is_none() && encoded.get("callerId").is_none());
    });
    retained_message!(
        proto::EnrollResponse,
        proto::EnrollResponseOwnedView,
        json!({"peerId": "peer_alpha", "attachment": fixtures::attachment()}),
        |handle| {
            assert_eq!(handle.peer_id(), Some("peer_alpha"));
            assert_eq!(
                handle.attachment().as_option().unwrap().attachment_ref.peer_id,
                handle.peer_id()
            );
            assert_eq!(
                handle.attachment().as_option().unwrap().attachment_ref.generation,
                Some(7)
            );
        }
    );
}

#[test]
fn heartbeat_and_detach_request_pairs_preserve_attachment_fencing() {
    retained_message!(
        proto::HeartbeatRequest,
        proto::HeartbeatRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref(), "availability": "PEER_AVAILABILITY_BLOCKED", "condition": "PEER_CONDITION_INPUT", "activeAt": fixtures::OBSERVED}),
        |handle| {
            assert_eq!(
                handle.attachment_ref().as_option().unwrap().connector_id,
                Some("connector_alpha")
            );
            assert_eq!(
                handle.availability().and_then(|value| value.as_known()),
                Some(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
            );
            assert_eq!(
                handle.condition().and_then(|value| value.as_known()),
                Some(proto::PeerCondition::PEER_CONDITION_INPUT)
            );
            assert!(handle.active_at().is_set());
        }
    );
    retained_message!(
        proto::HeartbeatResponse,
        proto::HeartbeatResponseOwnedView,
        json!({"attachment": fixtures::attachment()}),
        |handle| {
            assert_eq!(
                handle.attachment().as_option().unwrap().attachment_ref.generation,
                Some(7)
            );
            assert!(handle.attachment().as_option().unwrap().expires_at.is_set());
        }
    );
    retained_message!(
        proto::DetachRequest,
        proto::DetachRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref()}),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert_eq!(handle.attachment_ref().as_option().unwrap().generation, Some(7));
        }
    );
    retained_message!(
        proto::DetachResponse,
        proto::DetachResponseOwnedView,
        json!({}),
        |handle| {
            assert_eq!(serde_json::to_value(handle.to_owned_message()).unwrap(), json!({}));
        }
    );
}

#[test]
fn roster_wire_keeps_page_cursor_and_each_rows_liveness_observation() {
    retained_message!(
        proto::RosterRequest,
        proto::RosterRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref(), "pageSize": 16, "pageToken": "cursor_current"}),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert_eq!(handle.page_size(), Some(16));
            assert_eq!(handle.page_token(), Some("cursor_current"));
        }
    );
    let mut second = fixtures::presence();
    second["peerId"] = json!("peer_beta");
    second["displayName"] = json!("Implement");
    retained_message!(
        proto::RosterResponse,
        proto::RosterResponseOwnedView,
        json!({"peers": [fixtures::presence(), second], "nextPageToken": "cursor_next"}),
        |handle| {
            assert_eq!(handle.peers().len(), 2);
            assert_eq!(handle.peers()[0].peer_id, Some("peer_alpha"));
            assert_eq!(handle.peers()[1].peer_id, Some("peer_beta"));
            assert!(
                handle
                    .peers()
                    .iter()
                    .all(|peer| peer.observed_at.is_set() && peer.expires_at.is_set())
            );
            assert_eq!(handle.next_page_token(), Some("cursor_next"));
        }
    );
}

#[test]
fn send_and_status_pairs_preserve_original_message_and_retry_key() {
    retained_message!(
        proto::SendRequest,
        proto::SendRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref(), "recipientPeerId": "peer_beta", "idempotencyKey": "send_outgoing", "text": "hello peer", "replyToMessageId": "message_previous"}),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert_eq!(handle.recipient_peer_id(), Some("peer_beta"));
            assert_eq!(handle.idempotency_key(), Some("send_outgoing"));
            assert_eq!(handle.text(), Some("hello peer"));
            assert_eq!(handle.reply_to_message_id(), Some("message_previous"));
            assert!(
                serde_json::to_value(handle.view())
                    .unwrap()
                    .get("senderPeerId")
                    .is_none()
            );
        }
    );
    retained_message!(
        proto::SendResponse,
        proto::SendResponseOwnedView,
        json!({"message": fixtures::metadata()}),
        |handle| {
            let message = handle.message().as_option().unwrap();
            assert_eq!(message.message_id, Some("message_outgoing"));
            assert_eq!(message.sender_peer_id, Some("peer_alpha"));
            assert_eq!(message.recipient_peer_id, Some("peer_beta"));
        }
    );
    retained_message!(
        proto::StatusRequest,
        proto::StatusRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref(), "idempotencyKey": "send_outgoing"}),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().generation, Some(7));
            assert_eq!(handle.idempotency_key(), Some("send_outgoing"));
        }
    );
    retained_message!(
        proto::StatusResponse,
        proto::StatusResponseOwnedView,
        json!({"message": fixtures::metadata(), "status": "MESSAGE_STATUS_BATCH_PREPARED", "observedAt": fixtures::PREPARED}),
        |handle| {
            assert_eq!(
                handle.message().as_option().unwrap().message_id,
                Some("message_outgoing")
            );
            assert_eq!(
                handle.status().and_then(|value| value.as_known()),
                Some(proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED)
            );
            assert!(handle.observed_at().is_set());
        }
    );
}

#[test]
fn delivery_pairs_keep_preparation_and_application_acknowledgment_distinct() {
    retained_message!(
        proto::PrepareBatchRequest,
        proto::PrepareBatchRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref(), "promptId": "prompt_alpha"}),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert_eq!(handle.prompt_id(), Some("prompt_alpha"));
        }
    );
    retained_message!(
        proto::PrepareBatchResponse,
        proto::PrepareBatchResponseOwnedView,
        json!({"batch": fixtures::batch(), "messages": [fixtures::prepared_message("message_incoming_one"), fixtures::prepared_message("message_incoming_two")], "contentAvailability": "CONTENT_AVAILABILITY_AVAILABLE"}),
        |handle| {
            let batch = handle.batch().as_option().unwrap();
            assert_eq!(batch.batch_id, Some("batch_local"));
            assert!(!batch.acknowledged_at.is_set());
            assert_eq!(handle.messages().len(), batch.message_ids.len());
            for (message, assigned_id) in handle.messages().iter().zip(batch.message_ids.iter()) {
                assert_eq!(message.metadata.message_id, Some(*assigned_id));
                assert_eq!(message.text, Some("hello peer"));
            }
            assert_eq!(
                handle.content_availability().and_then(|value| value.as_known()),
                Some(proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE)
            );
        }
    );
    retained_message!(
        proto::AcknowledgeBatchRequest,
        proto::AcknowledgeBatchRequestOwnedView,
        json!({"attachmentRef": fixtures::attachment_ref(), "batchId": "batch_local"}),
        |handle| {
            assert_eq!(handle.attachment_ref().as_option().unwrap().peer_id, Some("peer_alpha"));
            assert_eq!(handle.batch_id(), Some("batch_local"));
        }
    );
    retained_message!(
        proto::AcknowledgeBatchResponse,
        proto::AcknowledgeBatchResponseOwnedView,
        json!({"batch": fixtures::acknowledged_batch()}),
        |handle| {
            let batch = handle.batch().as_option().unwrap();
            assert_eq!(batch.batch_id, Some("batch_local"));
            assert!(batch.prepared_at.is_set() && batch.acknowledged_at.is_set());
        }
    );
    let unavailable = assert_json_codec::<proto::PrepareBatchResponse>(json!({
        "batch": fixtures::batch(), "contentAvailability": "CONTENT_AVAILABILITY_RETRY_UNAVAILABLE"
    }));
    assert!(unavailable.messages.is_empty());
    assert_eq!(unavailable.batch.batch_id.as_deref(), Some("batch_local"));
}
