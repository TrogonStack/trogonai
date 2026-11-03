use super::{TenantId, MessageId};
use crate::MessageError;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};
#[derive(Debug, Clone)]
pub struct AuthoritativeReceipt { pub(crate) tenant: TenantId, pub(crate) receipt: proto::RecipientReceipt }
impl AuthoritativeReceipt {
    pub fn from_recipient_history(state: &PeerState, id: &MessageId) -> Result<Self,MessageError> {
        let receipt=state.dispositions.iter().find(|r|r.message_id.as_deref()==Some(id.as_str())).ok_or(MessageError::InvalidTransition)?;
        Ok(Self{tenant:TenantId::new(state.enrollment.tenant_id.clone().unwrap_or_default())?,receipt:receipt.clone()})
    }
    pub fn tenant(&self) -> &TenantId {&self.tenant}
    pub fn receipt(&self) -> &proto::RecipientReceipt {&self.receipt}
}
