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

pub(super) fn pending_inbox(state: &PeerState) -> Result<(usize, u64), EvolveError> {
    let mut count = 0;
    let mut bytes = 0;
    for entry in &state.inbox {
        metadata(
            entry
                .message
                .as_option()
                .ok_or(EvolveError::MissingField { field: "inbox.message" })?,
        )?;
        let pending = if let Some(receipt) = entry.disposition.as_option() {
            match validate_receipt(receipt)? {
                proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED => {
                    let batch = state
                        .batches
                        .iter()
                        .find(|b| b.batch_id == receipt.batch_id)
                        .ok_or(EvolveError::InvalidMessageTransition)?;
                    !batch.acknowledged_at.is_set()
                }
                _ => false,
            }
        } else {
            true
        };
        if pending {
            count += 1;
            bytes += u64::from(entry.message.content_size_bytes.unwrap_or(0));
        }
    }
    Ok((count, bytes))
}
pub(super) fn make_receipt(
    state: &PeerState,
    message: &proto::MessageMetadata,
    outcome: proto::PreparationOutcome,
    batch_id: Option<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<proto::RecipientReceipt, EvolveError> {
    let revision = u64::try_from(state.dispositions.len())
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or(EvolveError::InvalidMessageTransition)?;
    let receipt = proto::RecipientReceipt {
        message_id: message.message_id.clone(),
        recipient_peer_id: message.recipient_peer_id.clone(),
        outcome: Some(outcome.into()),
        batch_id,
        recipient_revision: Some(revision),
        decided_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&now)),
    };
    validate_receipt(&receipt)?;
    Ok(receipt)
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
pub(super) fn fold_intake(state: &mut PeerState, event: &proto::PeerMessageIntakeRecorded) -> Result<(), EvolveError> {
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    state_values::parse_enrollment(enrollment)?;
    let entry = event
        .entry
        .as_option()
        .ok_or(EvolveError::MissingField { field: "inbox.entry" })?;
    let message = entry
        .message
        .as_option()
        .ok_or(EvolveError::MissingField { field: "inbox.message" })?;
    metadata(message)?;
    if message.recipient_peer_id != enrollment.peer_id
        || message.collaboration_ref != enrollment.collaboration_ref
        || entry.disposition.is_set()
        || entry.intake_ordinal.unwrap_or(0) == 0
        || entry.intake_ordinal != state.next_intake_ordinal
        || state.inbox.iter().any(|e| e.message.message_id == message.message_id)
        || state.dispositions.iter().any(|r| r.message_id == message.message_id)
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let (count, bytes) = pending_inbox(state)?;
    if count >= 64 || bytes + u64::from(message.content_size_bytes.unwrap_or(0)) > 131072 {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let next = entry
        .intake_ordinal
        .and_then(|n| n.checked_add(1))
        .ok_or(EvolveError::InvalidMessageTransition)?;
    state.inbox.push(entry.clone());
    state.next_intake_ordinal = Some(next);
    Ok(())
}
pub(super) fn fold_receipt(state: &mut PeerState, receipt: &proto::RecipientReceipt) -> Result<(), EvolveError> {
    let outcome = validate_receipt(receipt)?;
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    state_values::parse_enrollment(enrollment)?;
    let next = u64::try_from(state.dispositions.len())
        .ok()
        .and_then(|n| n.checked_add(1));
    if receipt.recipient_peer_id != enrollment.peer_id
        || receipt.recipient_revision != next
        || state.dispositions.iter().any(|r| r.message_id == receipt.message_id)
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    if let Some(entry) = state
        .inbox
        .iter_mut()
        .find(|e| e.message.message_id == receipt.message_id)
    {
        if entry.disposition.is_set() {
            return Err(EvolveError::InvalidMessageTransition);
        }
        let at = state_values::read_time(receipt.decided_at.as_option(), "receipt.decided_at")?;
        let accepted = state_values::read_time(entry.message.accepted_at.as_option(), "accepted_at")?;
        let expires = state_values::read_time(entry.message.expires_at.as_option(), "expires_at")?;
        if at < accepted
            || (outcome == proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED && at < expires)
            || (outcome == proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED && at >= expires)
        {
            return Err(EvolveError::InvalidMessageTransition);
        }
        entry.disposition = buffa::MessageField::some(receipt.clone());
    } else if outcome == proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED {
        return Err(EvolveError::InvalidMessageTransition);
    }
    state.dispositions.push(receipt.clone());
    Ok(())
}
pub(super) fn fold_disposed(state: &mut PeerState, event: &proto::PeerMessageDisposed) -> Result<(), EvolveError> {
    let receipt = event
        .receipt
        .as_option()
        .ok_or(EvolveError::MissingField { field: "receipt" })?;
    if validate_receipt(receipt)? == proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED {
        return Err(EvolveError::InvalidMessageTransition);
    }
    fold_receipt(state, receipt)
}

pub(super) fn eligible_inbox(
    state: &PeerState,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<&proto::InboxEntry>, EvolveError> {
    if !(-62_135_596_800..=253_402_300_799).contains(&now.timestamp()) {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let mut eligible = Vec::new();
    for entry in &state.inbox {
        metadata(
            entry
                .message
                .as_option()
                .ok_or(EvolveError::MissingField { field: "inbox.message" })?,
        )?;
        if entry.intake_ordinal.unwrap_or(0) == 0 {
            return Err(EvolveError::InvalidMessageTransition);
        }
        let accepted = state_values::read_time(entry.message.accepted_at.as_option(), "accepted_at")?;
        let expires = state_values::read_time(entry.message.expires_at.as_option(), "expires_at")?;
        if !entry.disposition.is_set() && now >= accepted && now < expires {
            eligible.push(entry);
        }
    }
    eligible.sort_by_key(|e| e.intake_ordinal);
    if eligible
        .windows(2)
        .any(|pair| pair[0].intake_ordinal == pair[1].intake_ordinal)
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    Ok(eligible)
}
pub(super) fn valid_selection(eligible: &[&proto::InboxEntry], ids: &[String]) -> bool {
    !ids.is_empty()
        && ids.len() <= 4
        && ids.len() <= eligible.len()
        && ids
            .iter()
            .zip(eligible)
            .all(|(id, entry)| entry.message.message_id.as_ref() == Some(id))
}
pub(super) fn fold_prepared(state: &mut PeerState, event: &proto::PeerBatchPrepared) -> Result<(), EvolveError> {
    let batch = event
        .batch
        .as_option()
        .ok_or(EvolveError::MissingField { field: "batch" })?;
    value(
        BatchId::new(state_values::required(&batch.batch_id, "batch_id")?),
        "batch_id",
    )?;
    let now = state_values::read_time(batch.prepared_at.as_option(), "prepared_at")?;
    if batch.acknowledged_at.is_set()
        || state
            .batches
            .iter()
            .any(|b| !b.acknowledged_at.is_set() || b.batch_id == batch.batch_id)
        || event.receipts.len() != batch.message_ids.len()
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    state_values::parse_enrollment(enrollment)?;
    let attachment = state.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
    state_values::validate_attachment(enrollment, attachment)?;
    if attachment.status.and_then(|s| s.as_known()) != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
        || now < state_values::read_time(attachment.observed_at.as_option(), "observed_at")?
        || now >= state_values::read_time(attachment.expires_at.as_option(), "attachment.expires_at")?
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    if !valid_selection(&eligible_inbox(state, now)?, &batch.message_ids) {
        return Err(EvolveError::InvalidMessageTransition);
    }
    for (id, receipt) in batch.message_ids.iter().zip(&event.receipts) {
        if receipt.message_id.as_ref() != Some(id)
            || receipt.batch_id != batch.batch_id
            || receipt.decided_at != batch.prepared_at
            || validate_receipt(receipt)? != proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED
        {
            return Err(EvolveError::InvalidMessageTransition);
        }
        fold_receipt(state, receipt)?;
    }
    state.batches.push(batch.clone());
    Ok(())
}
