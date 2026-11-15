use serde_json::{Value, json};

pub(super) const OBSERVED: &str = "2026-11-02T12:00:00Z";
pub(super) const LEASE_END: &str = "2026-11-02T12:00:10Z";
pub(super) const PREPARED: &str = "2026-11-02T12:01:00Z";
pub(super) const ACKNOWLEDGED: &str = "2026-11-02T12:02:00Z";
pub(super) const DIGEST: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
pub(super) const SEMANTIC_FINGERPRINT: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

pub(super) fn attachment_ref() -> Value {
    json!({"peerId": "peer_alpha", "connectorId": "connector_alpha", "generation": "7"})
}

pub(super) fn attachment() -> Value {
    json!({
        "attachmentRef": attachment_ref(), "status": "ATTACHMENT_STATUS_ATTACHED",
        "observedAt": OBSERVED, "expiresAt": LEASE_END
    })
}

pub(super) fn enrollment() -> Value {
    json!({
        "peerId": "peer_alpha", "tenantId": "tenant_alpha", "callerId": "caller_alpha",
        "installationId": "installation_alpha", "nativeSessionId": "native_alpha",
        "collaborationRef": "collaboration_alpha", "displayName": "Review", "enrolledAt": OBSERVED
    })
}

pub(super) fn presence() -> Value {
    json!({
        "peerId": "peer_alpha", "displayName": "Review", "availability": "PEER_AVAILABILITY_BLOCKED",
        "condition": "PEER_CONDITION_RATE_LIMIT", "deliveryCapability": "DELIVERY_CAPABILITY_ON_USER_PROMPT",
        "observedAt": OBSERVED, "expiresAt": LEASE_END, "activeAt": "2026-11-02T11:59:59Z"
    })
}

pub(super) fn content_ref() -> Value {
    json!({
        "storageRef": {"value": "content_outgoing"},
        "digest": {"algorithm": "sha256", "value": DIGEST}
    })
}

pub(super) fn semantic_fingerprint() -> Value {
    json!({"value": SEMANTIC_FINGERPRINT})
}

pub(super) fn metadata() -> Value {
    json!({
        "messageId": "message_outgoing", "senderPeerId": "peer_alpha", "recipientPeerId": "peer_beta",
        "collaborationRef": "collaboration_alpha", "contentRef": content_ref(),
        "semanticFingerprint": semantic_fingerprint(), "contentSizeBytes": 10,
        "acceptedAt": OBSERVED, "expiresAt": "2026-11-02T13:00:00Z",
        "replyToMessageId": "message_previous"
    })
}

pub(super) fn incoming_metadata(id: &str) -> Value {
    let mut value = metadata();
    value["messageId"] = json!(id);
    value["senderPeerId"] = json!("peer_beta");
    value["recipientPeerId"] = json!("peer_alpha");
    value["contentRef"]["storageRef"]["value"] = json!(format!("content_{id}"));
    value
}

pub(super) fn receipt() -> Value {
    json!({
        "messageId": "message_outgoing", "recipientPeerId": "peer_beta",
        "outcome": "PREPARATION_OUTCOME_BATCH_PREPARED", "batchId": "batch_remote",
        "recipientRevision": "9007199254740993", "decidedAt": PREPARED
    })
}

pub(super) fn incoming_receipt(id: &str, revision: u64) -> Value {
    let mut value = receipt();
    value["messageId"] = json!(id);
    value["recipientPeerId"] = json!("peer_alpha");
    value["batchId"] = json!("batch_local");
    value["recipientRevision"] = json!(revision.to_string());
    value
}

pub(super) fn outbox() -> Value {
    json!({"idempotencyKey": "send_outgoing", "semanticDigest": DIGEST, "message": metadata(), "settlement": receipt()})
}

pub(super) fn inbox(id: &str, ordinal: u64) -> Value {
    json!({"message": incoming_metadata(id), "intakeOrdinal": ordinal.to_string(), "disposition": incoming_receipt(id, ordinal + 4)})
}

pub(super) fn batch() -> Value {
    json!({
        "batchId": "batch_local", "messageIds": ["message_incoming_one", "message_incoming_two"],
        "preparedAt": PREPARED
    })
}

pub(super) fn acknowledged_batch() -> Value {
    let mut value = batch();
    value["acknowledgedAt"] = json!(ACKNOWLEDGED);
    value
}

pub(super) fn expired_receipt() -> Value {
    json!({
        "messageId": "message_expired", "recipientPeerId": "peer_alpha",
        "outcome": "PREPARATION_OUTCOME_EXPIRED", "recipientRevision": "10", "decidedAt": PREPARED
    })
}

pub(super) fn prepared_message(id: &str) -> Value {
    json!({"metadata": incoming_metadata(id), "text": "hello peer"})
}

pub(super) fn state() -> Value {
    json!({
        "enrollment": enrollment(), "attachment": attachment(), "availability": "PEER_AVAILABILITY_BLOCKED",
        "condition": "PEER_CONDITION_RATE_LIMIT", "activeAt": OBSERVED, "outbox": [outbox()],
        "inbox": [inbox("message_incoming_one", 7), inbox("message_incoming_two", 8)],
        "batches": [acknowledged_batch()], "dispositions": [expired_receipt()], "nextIntakeOrdinal": "9"
    })
}

pub(super) fn events() -> Vec<Value> {
    vec![
        json!({"peerEnrolled": {"enrollment": enrollment()}}),
        json!({"peerAttached": {"attachment": attachment()}}),
        json!({"peerHeartbeatRecorded": {"attachment": attachment(), "availability": "PEER_AVAILABILITY_BLOCKED", "condition": "PEER_CONDITION_RATE_LIMIT", "activeAt": OBSERVED}}),
        json!({"peerDetached": {"attachmentRef": attachment_ref(), "detachedAt": LEASE_END}}),
        json!({"peerMessageAccepted": {"entry": outbox()}}),
        json!({"peerMessageIntakeRecorded": {"entry": inbox("message_incoming_one", 7)}}),
        json!({"peerBatchPrepared": {"batch": batch(), "receipts": [incoming_receipt("message_incoming_one", 11), incoming_receipt("message_incoming_two", 12)]}}),
        json!({"peerBatchAcknowledged": {"batchId": "batch_local", "acknowledgedAt": ACKNOWLEDGED}}),
        json!({"peerMessageDisposed": {"receipt": expired_receipt()}}),
        json!({"peerMessageSettled": {"receipt": receipt()}}),
    ]
}
