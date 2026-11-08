use super::{
    AcceptedMessage, IntakenMessage, domain::*, evolve_error::EvolveError, inbox_replacement::InboxReplacement,
    accepted_outbox::AcceptedOutbox, state_values,
};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

use super::message_values::value;
pub(super) use super::message_values::{metadata, outbox_entry, receipt_matches_message, semantic_digest, validate_receipt};












pub(super) fn fold_accepted(
    state: &mut PeerState,
    event: &proto::PeerMessageAccepted,
) -> Result<AcceptedOutbox, EvolveError> {
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    let facts = state_values::parse_enrollment_facts(enrollment)?;
    let entry = event
        .entry
        .as_option()
        .ok_or(EvolveError::MissingField { field: "entry" })?;
    let parsed = outbox_entry(entry)?;
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
    let evidence = AcceptedMessage {
        tenant: facts.identity().owner().tenant().clone(),
        recipient: parsed.metadata.recipient,
        message: parsed.metadata.message.clone(),
        content: parsed.metadata.content,
    };
    state.outbox.push(entry.clone());
    Ok(AcceptedOutbox {
        key: parsed.key,
        evidence,
    })
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
    if !state_values::timestamp_seconds_supported(now.timestamp()) || now.timestamp_subsec_nanos() >= 1_000_000_000 {
        return Err(EvolveError::TimestampRange {
            field: "receipt.decided_at",
        });
    }
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

pub(super) fn fold_intake(
    state: &mut PeerState,
    event: &proto::PeerMessageIntakeRecorded,
) -> Result<IntakenMessage, EvolveError> {
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
    let parsed = metadata(message)?;
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
    let intaken = IntakenMessage {
        entry: entry.clone(),
        metadata: parsed.message.clone(),
        message_id: parsed.message_id,
        content: parsed.content,
        accepted_at: parsed.accepted_at,
        expires_at: parsed.expires_at,
    };
    state.inbox.push(entry.clone());
    state.next_intake_ordinal = Some(next);
    Ok(intaken)
}
pub(super) fn fold_receipt(
    state: &mut PeerState,
    receipt: &proto::RecipientReceipt,
) -> Result<Vec<InboxReplacement>, EvolveError> {
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
    let mut replacements = Vec::new();
    if let Some((slot, entry)) = state
        .inbox
        .iter_mut()
        .enumerate()
        .find(|(_, e)| e.message.message_id == receipt.message_id)
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
        replacements.push(InboxReplacement {
            slot,
            entry: entry.clone(),
        });
    }
    state.dispositions.push(receipt.clone());
    Ok(replacements)
}
pub(super) fn fold_disposed(
    state: &mut PeerState,
    event: &proto::PeerMessageDisposed,
) -> Result<Vec<InboxReplacement>, EvolveError> {
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
pub(super) fn fold_prepared(
    state: &mut PeerState,
    event: &proto::PeerBatchPrepared,
) -> Result<Vec<InboxReplacement>, EvolveError> {
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
    let mut replacements = Vec::new();
    for (id, receipt) in batch.message_ids.iter().zip(&event.receipts) {
        if receipt.message_id.as_ref() != Some(id)
            || receipt.batch_id != batch.batch_id
            || receipt.decided_at != batch.prepared_at
            || validate_receipt(receipt)? != proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED
        {
            return Err(EvolveError::InvalidMessageTransition);
        }
        replacements.extend(fold_receipt(state, receipt)?);
    }
    state.batches.push(batch.clone());
    Ok(replacements)
}


pub(super) fn fold_acknowledged(
    state: &mut PeerState,
    event: &proto::PeerBatchAcknowledged,
) -> Result<(), EvolveError> {
    value(
        BatchId::new(state_values::required(&event.batch_id, "batch_id")?),
        "batch_id",
    )?;
    let now = state_values::read_time(event.acknowledged_at.as_option(), "acknowledged_at")?;
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    state_values::parse_enrollment(enrollment)?;
    let attachment = state.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
    state_values::validate_attachment(enrollment, attachment)?;
    if attachment.status.and_then(|s| s.as_known()) != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
        || now < state_values::read_time(attachment.observed_at.as_option(), "observed_at")?
        || now >= state_values::read_time(attachment.expires_at.as_option(), "expires_at")?
    {
        return Err(EvolveError::InvalidMessageTransition);
    }
    let batch = state
        .batches
        .iter_mut()
        .find(|b| b.batch_id == event.batch_id)
        .ok_or(EvolveError::InvalidMessageTransition)?;
    if batch.acknowledged_at.is_set() || now < state_values::read_time(batch.prepared_at.as_option(), "prepared_at")? {
        return Err(EvolveError::InvalidMessageTransition);
    }
    batch.acknowledged_at = event.acknowledged_at.clone();
    Ok(())
}
pub(super) fn fold_settled(state: &mut PeerState, event: &proto::PeerMessageSettled) -> Result<(), EvolveError> {
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    state_values::parse_enrollment(enrollment)?;
    let receipt = event
        .receipt
        .as_option()
        .ok_or(EvolveError::MissingField { field: "receipt" })?;
    validate_receipt(receipt)?;
    let entry = state
        .outbox
        .iter_mut()
        .find(|e| e.message.message_id == receipt.message_id)
        .ok_or(EvolveError::InvalidMessageTransition)?;
    outbox_entry(entry)?;
    receipt_matches_message(receipt, &entry.message)?;
    if entry.settlement.is_set() || entry.message.sender_peer_id != enrollment.peer_id {
        return Err(EvolveError::InvalidMessageTransition);
    }
    entry.settlement = buffa::MessageField::some(receipt.clone());
    Ok(())
}
