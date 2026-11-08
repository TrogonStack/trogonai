use super::{domain::*, evolve_error::EvolveError, state_values};
use sha2::{Digest, Sha256};
use trogonai_proto::peers::v1alpha1 as proto;

pub(super) fn value<T>(result: Result<T, PeerValueError>, field: &'static str) -> Result<T, EvolveError> {
    result.map_err(|source| EvolveError::InvalidValue { field, source })
}
pub(super) struct ParsedMetadata<'a> {
    pub(super) message: &'a proto::MessageMetadata,
    pub(super) sender: PeerId,
    pub(super) recipient: PeerId,
    pub(super) collaboration: CollaborationRef,
    pub(super) content: TrustedContent,
}

pub(super) struct ParsedOutbox<'a> {
    pub(super) key: IdempotencyKey,
    pub(super) metadata: ParsedMetadata<'a>,
}

pub(super) fn metadata(message: &proto::MessageMetadata) -> Result<ParsedMetadata<'_>, EvolveError> {
    value(
        MessageId::new(state_values::required(&message.message_id, "message_id")?),
        "message_id",
    )?;
    let sender = value(
        PeerId::parse(state_values::required(&message.sender_peer_id, "sender_peer_id")?),
        "sender_peer_id",
    )?;
    let recipient = value(
        PeerId::parse(state_values::required(&message.recipient_peer_id, "recipient_peer_id")?),
        "recipient_peer_id",
    )?;
    let collaboration = value(
        CollaborationRef::new(state_values::required(&message.collaboration_ref, "collaboration_ref")?),
        "collaboration_ref",
    )?;
    let reference = value(
        SealedContentRef::new(
            message
                .content_ref
                .as_option()
                .ok_or(EvolveError::MissingField { field: "content_ref" })?
                .clone(),
        ),
        "content_ref",
    )?;
    let content = value(
        TrustedContent::from_sealed_port(
            reference,
            message.content_size_bytes.ok_or(EvolveError::MissingField {
                field: "content_size_bytes",
            })?,
            semantic_fingerprint(message)?.as_bytes().to_vec(),
        ),
        "content",
    )?;
    if let Some(reply) = &message.reply_to_message_id {
        value(MessageId::new(reply), "reply_to_message_id")?;
    }
    let accepted = state_values::read_time(message.accepted_at.as_option(), "accepted_at")?;
    let expires = state_values::read_time(message.expires_at.as_option(), "expires_at")?;
    if expires - accepted != chrono::Duration::hours(1) {
        return Err(EvolveError::InvalidMessageTransition);
    }
    Ok(ParsedMetadata {
        message,
        sender,
        recipient,
        collaboration,
        content,
    })
}

fn semantic_fingerprint(message: &proto::MessageMetadata) -> Result<ContentSemanticFingerprint, EvolveError> {
    let fingerprint = message
        .semantic_fingerprint
        .as_option()
        .ok_or(EvolveError::MissingField {
            field: "semantic_fingerprint",
        })?;
    value(
        ContentSemanticFingerprint::new(fingerprint.value.clone().ok_or(EvolveError::MissingField {
            field: "semantic_fingerprint.value",
        })?),
        "semantic_fingerprint",
    )
}

pub(super) fn semantic_digest(message: &proto::MessageMetadata) -> Result<Vec<u8>, EvolveError> {
    let fingerprint = semantic_fingerprint(message)?;
    let mut hash = Sha256::new();
    for bytes in [
        b"peer-message-contract-v1".as_slice(),
        message.sender_peer_id.as_deref().unwrap_or_default().as_bytes(),
        message.recipient_peer_id.as_deref().unwrap_or_default().as_bytes(),
        message.collaboration_ref.as_deref().unwrap_or_default().as_bytes(),
        fingerprint.as_bytes(),
        &message.content_size_bytes.unwrap_or(0).to_be_bytes(),
        message.reply_to_message_id.as_deref().unwrap_or_default().as_bytes(),
    ] {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    Ok(hash.finalize().to_vec())
}
pub(super) fn outbox_entry(entry: &proto::OutboxEntry) -> Result<ParsedOutbox<'_>, EvolveError> {
    let key = value(
        IdempotencyKey::new(state_values::required(&entry.idempotency_key, "idempotency_key")?),
        "idempotency_key",
    )?;
    let message = entry
        .message
        .as_option()
        .ok_or(EvolveError::MissingField { field: "message" })?;
    let metadata = metadata(message)?;
    if entry.semantic_digest.as_deref() != Some(semantic_digest(message)?.as_slice()) {
        return Err(EvolveError::InvalidMessageTransition);
    }
    if let Some(receipt) = entry.settlement.as_option() {
        receipt_matches_message(receipt, message)?;
    }
    Ok(ParsedOutbox { key, metadata })
}
pub(super) fn validate_receipt(receipt: &proto::RecipientReceipt) -> Result<proto::PreparationOutcome, EvolveError> {
    value(
        MessageId::new(state_values::required(&receipt.message_id, "receipt.message_id")?),
        "receipt.message_id",
    )?;
    value(
        PeerId::parse(state_values::required(
            &receipt.recipient_peer_id,
            "receipt.recipient_peer_id",
        )?),
        "receipt.recipient_peer_id",
    )?;
    if receipt.recipient_revision.unwrap_or(0) == 0 {
        return Err(EvolveError::InvalidMessageTransition);
    }
    state_values::read_time(receipt.decided_at.as_option(), "receipt.decided_at")?;
    let outcome = receipt
        .outcome
        .and_then(|o| o.as_known())
        .ok_or(EvolveError::InvalidMessageTransition)?;
    match outcome {
        proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED => {
            value(
                BatchId::new(state_values::required(&receipt.batch_id, "receipt.batch_id")?),
                "receipt.batch_id",
            )?;
        }
        proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED
        | proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE
            if receipt.batch_id.is_none() => {}
        _ => return Err(EvolveError::InvalidMessageTransition),
    }
    Ok(outcome)
}
pub(super) fn receipt_matches_message(
    receipt: &proto::RecipientReceipt,
    message: &proto::MessageMetadata,
) -> Result<(), EvolveError> {
    metadata(message)?;
    let outcome = validate_receipt(receipt)?;
    if receipt.message_id != message.message_id || receipt.recipient_peer_id != message.recipient_peer_id {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let at = state_values::read_time(receipt.decided_at.as_option(), "decided_at")?;
    let accepted = state_values::read_time(message.accepted_at.as_option(), "accepted_at")?;
    let expires = state_values::read_time(message.expires_at.as_option(), "expires_at")?;
    if at < accepted
        || (outcome == proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED && at < expires)
        || (outcome == proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED && at >= expires)
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    Ok(())
}
