use super::domain::{MessageId, PeerId, TenantId};
use super::{evolve_error::EvolveError, message_error::MessageError, message_rules, state_values};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};
#[derive(Debug, Clone)]
pub struct AcceptedMessage {
    pub(crate) tenant: TenantId,
    pub(crate) recipient: PeerId,
    pub(crate) message: proto::MessageMetadata,
}
impl AcceptedMessage {
    pub fn from_sender_history(state: &PeerState, id: &MessageId) -> Result<Self, MessageError> {
        let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
        let facts = state_values::parse_enrollment_facts(enrollment)?;
        let entry = state
            .outbox
            .iter()
            .find(|entry| entry.message.message_id.as_deref() == Some(id.as_str()))
            .ok_or(MessageError::InvalidTransition)?;
        let parsed = message_rules::outbox_entry(entry)?;
        if &parsed.metadata.sender != facts.identity().peer_id()
            || &parsed.metadata.collaboration != facts.collaboration()
        {
            return Err(MessageError::InvalidTransition);
        }
        Ok(Self {
            tenant: facts.identity().owner().tenant().clone(),
            recipient: parsed.metadata.recipient,
            message: parsed.metadata.message.clone(),
        })
    }
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn recipient(&self) -> &PeerId {
        &self.recipient
    }
    pub fn metadata(&self) -> &proto::MessageMetadata {
        &self.message
    }
}
