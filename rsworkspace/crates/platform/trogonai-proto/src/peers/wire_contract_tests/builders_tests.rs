use std::fmt::Debug;

use buffa::Message;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::fixtures;
use crate::peers::{state_v1alpha1 as state, v1alpha1 as proto};

fn assert_builder<M: Message + Serialize + DeserializeOwned + Debug + PartialEq>(built: M, fixture: Value) {
    let decoded: M = serde_json::from_value(fixture.clone()).unwrap();
    assert_eq!(built, decoded);
    assert_eq!(built.encode_to_vec(), decoded.encode_to_vec());
    assert_eq!(serde_json::to_value(built).unwrap(), fixture);
}

fn nested<M: DeserializeOwned>(fixture: Value) -> M {
    serde_json::from_value(fixture).unwrap()
}

#[test]
fn identity_builders_match_wire_fields_and_replace_only_the_selected_value() {
    let reference = proto::AttachmentRef::default()
        .with_peer_id("discarded_peer")
        .with_connector_id("connector_alpha")
        .with_generation(7)
        .with_peer_id(String::from("peer_alpha"));
    assert_builder(reference, fixtures::attachment_ref());
    let enrollment: proto::PeerEnrollment = nested(json!({"enrolledAt": fixtures::OBSERVED}));
    let enrollment = enrollment
        .with_peer_id("peer_alpha")
        .with_tenant_id("tenant_alpha")
        .with_caller_id("caller_alpha")
        .with_installation_id("installation_alpha")
        .with_native_session_id("native_alpha")
        .with_collaboration_ref("collaboration_alpha")
        .with_display_name("Review");
    assert_builder(enrollment, fixtures::enrollment());
    let attachment: proto::PeerAttachment = nested(json!({
        "attachmentRef": fixtures::attachment_ref(), "observedAt": fixtures::OBSERVED, "expiresAt": fixtures::LEASE_END
    }));
    assert_builder(
        attachment.with_status(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED),
        fixtures::attachment(),
    );
    let presence: proto::PeerPresence = nested(json!({
        "observedAt": fixtures::OBSERVED, "expiresAt": fixtures::LEASE_END, "activeAt": "2026-11-02T11:59:59Z"
    }));
    let presence = presence
        .with_peer_id("peer_alpha")
        .with_display_name("Review")
        .with_availability(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
        .with_condition(proto::PeerCondition::PEER_CONDITION_RATE_LIMIT)
        .with_delivery_capability(proto::DeliveryCapability::DELIVERY_CAPABILITY_ON_USER_PROMPT);
    assert_builder(presence, fixtures::presence());
}

#[test]
fn message_builders_keep_the_content_reference_digest_and_reply_provenance() {
    let metadata: proto::MessageMetadata = nested(json!({
        "contentRef": fixtures::content_ref(), "semanticFingerprint": fixtures::semantic_fingerprint(),
        "acceptedAt": fixtures::OBSERVED, "expiresAt": "2026-11-02T13:00:00Z"
    }));
    let metadata = metadata
        .with_message_id("message_outgoing")
        .with_sender_peer_id("peer_alpha")
        .with_recipient_peer_id("peer_beta")
        .with_collaboration_ref("collaboration_alpha")
        .with_content_size_bytes(10)
        .with_reply_to_message_id("message_previous");
    assert_builder(metadata, fixtures::metadata());
    assert_builder(
        proto::MessageContent::default().with_text("hello peer"),
        json!({"text": "hello peer"}),
    );
    let prepared: proto::PreparedMessage =
        nested(json!({"metadata": fixtures::incoming_metadata("message_incoming_one")}));
    assert_builder(
        prepared.with_text("hello peer"),
        fixtures::prepared_message("message_incoming_one"),
    );
}

#[test]
fn receipt_and_mailbox_builders_preserve_deduplication_and_progress_coordinates() {
    let receipt: proto::RecipientReceipt = nested(json!({"decidedAt": fixtures::PREPARED}));
    let receipt = receipt
        .with_message_id("message_outgoing")
        .with_recipient_peer_id("peer_beta")
        .with_outcome(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED)
        .with_batch_id("batch_remote")
        .with_recipient_revision(9_007_199_254_740_993);
    assert_builder(receipt, fixtures::receipt());
    let outbox: proto::OutboxEntry =
        nested(json!({"message": fixtures::metadata(), "settlement": fixtures::receipt()}));
    assert_builder(
        outbox
            .with_idempotency_key("send_outgoing")
            .with_semantic_digest(vec![0; 32]),
        fixtures::outbox(),
    );
    let inbox: proto::InboxEntry = nested(json!({
        "message": fixtures::incoming_metadata("message_incoming_one"), "disposition": fixtures::incoming_receipt("message_incoming_one", 11)
    }));
    assert_builder(inbox.with_intake_ordinal(7), fixtures::inbox("message_incoming_one", 7));
    let batch: proto::PeerBatch = nested(json!({
        "messageIds": ["message_incoming_one", "message_incoming_two"], "preparedAt": fixtures::PREPARED
    }));
    assert_builder(batch.with_batch_id("batch_local"), fixtures::batch());
    let mut partial = fixtures::state();
    partial.as_object_mut().unwrap().remove("availability");
    partial.as_object_mut().unwrap().remove("condition");
    partial.as_object_mut().unwrap().remove("nextIntakeOrdinal");
    let state: state::PeerState = nested(partial);
    let state = state
        .with_availability(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
        .with_condition(proto::PeerCondition::PEER_CONDITION_RATE_LIMIT)
        .with_next_intake_ordinal(9);
    assert_builder(state, fixtures::state());
}

#[test]
fn heartbeat_and_acknowledgment_fact_builders_do_not_change_attachment_or_time_fields() {
    let heartbeat: proto::PeerHeartbeatRecorded =
        nested(json!({"attachment": fixtures::attachment(), "activeAt": fixtures::OBSERVED}));
    assert_builder(
        heartbeat
            .with_availability(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
            .with_condition(proto::PeerCondition::PEER_CONDITION_RATE_LIMIT),
        json!({"attachment": fixtures::attachment(), "availability": "PEER_AVAILABILITY_BLOCKED", "condition": "PEER_CONDITION_RATE_LIMIT", "activeAt": fixtures::OBSERVED}),
    );
    let acknowledged: proto::PeerBatchAcknowledged = nested(json!({"acknowledgedAt": fixtures::ACKNOWLEDGED}));
    assert_builder(
        acknowledged.with_batch_id("batch_local"),
        json!({"batchId": "batch_local", "acknowledgedAt": fixtures::ACKNOWLEDGED}),
    );
}

#[test]
fn enrollment_and_query_builders_match_their_public_rpc_contracts() {
    let enroll = proto::EnrollRequest::default()
        .with_installation_id("installation_alpha")
        .with_native_session_id("native_alpha")
        .with_connector_id("connector_alpha")
        .with_collaboration_ref("collaboration_alpha")
        .with_display_name("Review");
    assert_builder(
        enroll,
        json!({"installationId": "installation_alpha", "nativeSessionId": "native_alpha", "connectorId": "connector_alpha", "collaborationRef": "collaboration_alpha", "displayName": "Review"}),
    );
    let enrolled: proto::EnrollResponse = nested(json!({"attachment": fixtures::attachment()}));
    assert_builder(
        enrolled.with_peer_id("peer_alpha"),
        json!({"peerId": "peer_alpha", "attachment": fixtures::attachment()}),
    );
    let heartbeat: proto::HeartbeatRequest =
        nested(json!({"attachmentRef": fixtures::attachment_ref(), "activeAt": fixtures::OBSERVED}));
    assert_builder(
        heartbeat
            .with_availability(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED)
            .with_condition(proto::PeerCondition::PEER_CONDITION_INPUT),
        json!({"attachmentRef": fixtures::attachment_ref(), "availability": "PEER_AVAILABILITY_BLOCKED", "condition": "PEER_CONDITION_INPUT", "activeAt": fixtures::OBSERVED}),
    );
    let roster: proto::RosterRequest = nested(json!({"attachmentRef": fixtures::attachment_ref()}));
    assert_builder(
        roster.with_page_size(16).with_page_token("cursor_current"),
        json!({"attachmentRef": fixtures::attachment_ref(), "pageSize": 16, "pageToken": "cursor_current"}),
    );
    let roster: proto::RosterResponse = nested(json!({"peers": [fixtures::presence()]}));
    assert_builder(
        roster.with_next_page_token("cursor_next"),
        json!({"peers": [fixtures::presence()], "nextPageToken": "cursor_next"}),
    );
}

#[test]
fn message_rpc_builders_keep_target_retry_key_and_batch_coordinates() {
    let send: proto::SendRequest = nested(json!({"attachmentRef": fixtures::attachment_ref()}));
    let send = send
        .with_recipient_peer_id("peer_beta")
        .with_idempotency_key("send_outgoing")
        .with_text("hello peer")
        .with_reply_to_message_id("message_previous");
    assert_builder(
        send,
        json!({"attachmentRef": fixtures::attachment_ref(), "recipientPeerId": "peer_beta", "idempotencyKey": "send_outgoing", "text": "hello peer", "replyToMessageId": "message_previous"}),
    );
    let status: proto::StatusRequest = nested(json!({"attachmentRef": fixtures::attachment_ref()}));
    assert_builder(
        status.with_idempotency_key("send_outgoing"),
        json!({"attachmentRef": fixtures::attachment_ref(), "idempotencyKey": "send_outgoing"}),
    );
    let status: proto::StatusResponse =
        nested(json!({"message": fixtures::metadata(), "observedAt": fixtures::PREPARED}));
    assert_builder(
        status.with_status(proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED),
        json!({"message": fixtures::metadata(), "status": "MESSAGE_STATUS_BATCH_PREPARED", "observedAt": fixtures::PREPARED}),
    );
    let prepare: proto::PrepareBatchRequest = nested(json!({"attachmentRef": fixtures::attachment_ref()}));
    assert_builder(
        prepare.with_prompt_id("prompt_alpha"),
        json!({"attachmentRef": fixtures::attachment_ref(), "promptId": "prompt_alpha"}),
    );
    let prepared: proto::PrepareBatchResponse = nested(json!({"batch": fixtures::batch()}));
    assert_builder(
        prepared.with_content_availability(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE),
        json!({"batch": fixtures::batch(), "contentAvailability": "CONTENT_AVAILABILITY_RETRY_UNAVAILABLE"}),
    );
    let acknowledge: proto::AcknowledgeBatchRequest = nested(json!({"attachmentRef": fixtures::attachment_ref()}));
    assert_builder(
        acknowledge.with_batch_id("batch_local"),
        json!({"attachmentRef": fixtures::attachment_ref(), "batchId": "batch_local"}),
    );
}

#[test]
fn scalar_builders_preserve_present_zero_and_empty_values_instead_of_absence() {
    let absent = proto::AttachmentRef::default();
    let zero = proto::AttachmentRef::default()
        .with_peer_id("")
        .with_connector_id("")
        .with_generation(0);
    assert_eq!(absent.generation, None);
    assert_eq!(zero.generation, Some(0));
    assert_ne!(zero.encode_to_vec(), absent.encode_to_vec());
    assert_builder(zero, json!({"peerId": "", "connectorId": "", "generation": "0"}));
    let empty = proto::MessageMetadata::default().with_content_size_bytes(0);
    assert_builder(empty, json!({"contentSizeBytes": 0}));
    let empty_digest = proto::Digest::default().with_value(Vec::<u8>::new());
    assert_eq!(empty_digest.value, Some(Vec::new()));
    assert_builder(empty_digest, json!({"value": ""}));
    assert_builder(
        state::PeerState::default().with_next_intake_ordinal(0),
        json!({"nextIntakeOrdinal": "0"}),
    );
}

#[test]
fn claim_check_construction_keeps_the_storage_locator_and_digest_together() {
    let semantic = proto::ContentSemanticFingerprint::default()
        .with_value(vec![9; 32])
        .with_value(vec![1; 32]);
    assert_builder(semantic, fixtures::semantic_fingerprint());
    let locator = proto::StorageLocator::default()
        .with_value("discarded")
        .with_value("content_outgoing");
    let digest = proto::Digest::default()
        .with_algorithm("sha256")
        .with_value(vec![0; 32]);
    assert_builder(locator.clone(), json!({"value": "content_outgoing"}));
    assert_builder(
        digest.clone(),
        json!({"algorithm": "sha256", "value": fixtures::DIGEST}),
    );
    let reference = proto::ClaimCheck {
        storage_ref: buffa::MessageField::some(locator),
        digest: buffa::MessageField::some(digest),
    };
    assert_builder(reference, fixtures::content_ref());
}
