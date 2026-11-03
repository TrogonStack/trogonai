use trogonai_proto::peers::state_v1alpha1::PeerState;

use super::accepted_message::AcceptedMessage;
use super::{evolve_error::EvolveError, state_values};

pub(super) fn recipient_evidence(
    state: &PeerState,
    evidence: &AcceptedMessage,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), super::MessageError> {
    let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
    let identity = state_values::parse_enrollment(enrollment)?;
    if identity.peer_id() != &evidence.recipient {
        return Err(super::MessageError::RecipientMismatch);
    }
    if identity.owner().tenant() != &evidence.tenant {
        return Err(super::MessageError::TenantMismatch);
    }
    if enrollment.collaboration_ref != evidence.message.collaboration_ref {
        return Err(super::MessageError::CollaborationMismatch);
    }
    let accepted = state_values::read_time(evidence.message.accepted_at.as_option(), "accepted_at")?;
    if now < accepted || !(-62_135_596_800..=253_402_300_799).contains(&now.timestamp()) {
        return Err(super::MessageError::InvalidTransition);
    }
    Ok(())
}
pub(super) fn prior_intake(state: &PeerState, evidence: &AcceptedMessage) -> Result<(), super::MessageError> {
    let prior = state
        .inbox
        .iter()
        .find(|e| e.message.message_id == evidence.message.message_id);
    if prior.is_some_and(|e| e.message.as_option() != Some(&evidence.message)) {
        return Err(super::MessageError::Conflict);
    }
    if let Some(receipt) = state
        .dispositions
        .iter()
        .find(|r| r.message_id == evidence.message.message_id)
    {
        return Err(super::MessageError::AlreadyDisposed(Box::new(receipt.clone())));
    }
    if let Some(entry) = prior {
        return Err(super::MessageError::AlreadyIntaken(Box::new(entry.clone())));
    }
    Ok(())
}
