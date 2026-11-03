use super::{PeerId, TenantId, MessageId};
use crate::MessageError;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};
#[derive(Debug, Clone)]
pub struct AcceptedMessage { pub(crate) tenant: TenantId, pub(crate) recipient: PeerId, pub(crate) message: proto::MessageMetadata }
impl AcceptedMessage {
    pub fn from_sender_history(state: &PeerState, id: &MessageId) -> Result<Self, MessageError> {
        let entry=state.outbox.iter().find(|entry| entry.message.message_id.as_deref()==Some(id.as_str())).ok_or(MessageError::InvalidTransition)?;
        Ok(Self {tenant: TenantId::new(state.enrollment.tenant_id.clone().unwrap_or_default())?,recipient: PeerId::parse(entry.message.recipient_peer_id.clone().unwrap_or_default())?,message: entry.message.as_option().ok_or(MessageError::InvalidTransition)?.clone()})
    }
    pub fn tenant(&self) -> &TenantId { &self.tenant }
    pub fn metadata(&self) -> &proto::MessageMetadata { &self.message }
}
