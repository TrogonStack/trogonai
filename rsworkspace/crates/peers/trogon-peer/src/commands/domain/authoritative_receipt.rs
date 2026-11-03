use super::{MessageId, TenantId};
use crate::MessageError;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};
#[derive(Debug, Clone)]
pub struct AuthoritativeReceipt {
    pub(crate) tenant: TenantId,
    pub(crate) receipt: proto::RecipientReceipt,
}
impl AuthoritativeReceipt {
    pub fn from_recipient_history(state: &PeerState, id: &MessageId) -> Result<Self, MessageError> {
        let receipt = state
            .dispositions
            .iter()
            .find(|r| r.message_id.as_deref() == Some(id.as_str()))
            .ok_or(MessageError::InvalidTransition)?;
        let enrollment = state
            .enrollment
            .as_option()
            .ok_or(super::super::EvolveError::MissingEnrollment)?;
        super::super::state::parse_enrollment(enrollment)?;
        super::super::message_rules::validate_receipt(receipt)?;
        let position = state
            .dispositions
            .iter()
            .position(|r| r.message_id == receipt.message_id)
            .ok_or(MessageError::InvalidTransition)?;
        if receipt.recipient_peer_id != enrollment.peer_id || receipt.recipient_revision != Some(position as u64 + 1) {
            return Err(MessageError::InvalidTransition);
        }
        if let Some(entry) = state.inbox.iter().find(|e| e.message.message_id == receipt.message_id) {
            if entry.disposition.as_option() != Some(receipt) {
                return Err(MessageError::InvalidTransition);
            }
            super::super::message_rules::receipt_matches_message(receipt, &entry.message)?;
        }
        if receipt.outcome.and_then(|o| o.as_known())
            == Some(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED)
        {
            let batch = state
                .batches
                .iter()
                .find(|b| b.batch_id == receipt.batch_id)
                .ok_or(MessageError::InvalidTransition)?;
            if !batch
                .message_ids
                .iter()
                .any(|id| receipt.message_id.as_ref() == Some(id))
                || batch.prepared_at != receipt.decided_at
                || !state.inbox.iter().any(|e| e.message.message_id == receipt.message_id)
            {
                return Err(MessageError::InvalidTransition);
            }
        }
        Ok(Self {
            tenant: TenantId::new(state.enrollment.tenant_id.clone().unwrap_or_default())?,
            receipt: receipt.clone(),
        })
    }
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn receipt(&self) -> &proto::RecipientReceipt {
        &self.receipt
    }
}
