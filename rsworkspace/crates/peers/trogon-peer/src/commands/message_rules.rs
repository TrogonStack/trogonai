use super::{domain::*, evolve_error::EvolveError, state_values};
use sha2::{Digest, Sha256};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

fn value<T>(result: Result<T, PeerValueError>, field: &'static str) -> Result<T, EvolveError> {
    result.map_err(|source| EvolveError::InvalidValue { field, source })
}
pub(super) fn metadata(message: &proto::MessageMetadata) -> Result<(), EvolveError> {
    value(
        MessageId::new(state_values::required(&message.message_id, "message_id")?),
        "message_id",
    )?;
    value(
        PeerId::parse(state_values::required(&message.sender_peer_id, "sender_peer_id")?),
        "sender_peer_id",
    )?;
    value(
        PeerId::parse(state_values::required(&message.recipient_peer_id, "recipient_peer_id")?),
        "recipient_peer_id",
    )?;
    value(
        CollaborationRef::new(state_values::required(&message.collaboration_ref, "collaboration_ref")?),
        "collaboration_ref",
    )?;
    let reference = value(
        SealedContentRef::new(state_values::required(&message.content_ref, "content_ref")?),
        "content_ref",
    )?;
    value(
        TrustedContent::from_sealed_port(
            reference,
            message.content_size_bytes.ok_or(EvolveError::MissingField {
                field: "content_size_bytes",
            })?,
            message.content_digest.clone().ok_or(EvolveError::MissingField {
                field: "content_digest",
            })?,
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
    Ok(())
}

pub(super) fn semantic_digest(message: &proto::MessageMetadata) -> Vec<u8> {
    let mut hash = Sha256::new();
    for bytes in [
        b"peer-message-contract-v1".as_slice(),
        message.sender_peer_id.as_deref().unwrap_or_default().as_bytes(),
        message.recipient_peer_id.as_deref().unwrap_or_default().as_bytes(),
        message.collaboration_ref.as_deref().unwrap_or_default().as_bytes(),
        message.content_digest.as_deref().unwrap_or_default(),
        &message.content_size_bytes.unwrap_or(0).to_be_bytes(),
        message.reply_to_message_id.as_deref().unwrap_or_default().as_bytes(),
    ] {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    hash.finalize().to_vec()
}
pub(super) fn outbox_entry(entry: &proto::OutboxEntry) -> Result<(), EvolveError> {
    value(
        IdempotencyKey::new(state_values::required(&entry.idempotency_key, "idempotency_key")?),
        "idempotency_key",
    )?;
    let message = entry
        .message
        .as_option()
        .ok_or(EvolveError::MissingField { field: "message" })?;
    metadata(message)?;
    if entry.semantic_digest.as_deref() != Some(semantic_digest(message).as_slice()) {
        return Err(EvolveError::InvalidMessageTransition);
    }
    Ok(())
}
pub(super) fn fold_accepted(state: &mut PeerState, event: &proto::PeerMessageAccepted) -> Result<(), EvolveError> {
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    state_values::parse_enrollment(enrollment)?;
    let entry = event
        .entry
        .as_option()
        .ok_or(EvolveError::MissingField { field: "entry" })?;
    outbox_entry(entry)?;
    if entry.settlement.is_set()
        || entry.message.sender_peer_id != enrollment.peer_id
        || entry.message.collaboration_ref != enrollment.collaboration_ref
        || state.outbox.iter().any(|old| {
            old.idempotency_key == entry.idempotency_key || old.message.message_id == entry.message.message_id
        })
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let now = state_values::read_time(entry.message.accepted_at.as_option(), "accepted_at")?;
    let attachment = state.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
    state_values::validate_attachment(enrollment, attachment)?;
    let observed = state_values::read_time(attachment.observed_at.as_option(), "observed_at")?;
    let expires = state_values::read_time(attachment.expires_at.as_option(), "expires_at")?;
    if now < observed
        || now >= expires
        || attachment.status.and_then(|s| s.as_known()) != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let mut count = 0;
    let mut bytes = 0u64;
    let mut recent = 0;
    for old in &state.outbox {
        outbox_entry(old)?;
        if !old.settlement.is_set() {
            count += 1;
            bytes += u64::from(old.message.content_size_bytes.unwrap_or(0));
        }
        let accepted = state_values::read_time(old.message.accepted_at.as_option(), "accepted_at")?;
        if accepted > now {
            return Err(EvolveError::InvalidMessageTransition);
        }
        if now - accepted < chrono::Duration::seconds(60) {
            recent += 1;
        }
    }
    if count >= 16 || bytes + u64::from(entry.message.content_size_bytes.unwrap_or(0)) > 32768 || recent >= 10 {
        return Err(EvolveError::InvalidMessageTransition);
    }
    state.outbox.push(entry.clone());
    Ok(())
}
