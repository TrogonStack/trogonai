use super::{MessageId, PeerValueError};
#[derive(Debug, Clone)]
pub struct TrustedBatchPlan { pub(crate) message_ids: Vec<MessageId>, pub(crate) rendered_utf8_bytes: usize }
impl TrustedBatchPlan {
    pub fn from_renderer(message_ids: Vec<MessageId>, rendered_utf8_bytes: usize) -> Result<Self,PeerValueError> { Ok(Self{message_ids,rendered_utf8_bytes}) }
    pub fn message_ids(&self) -> &[MessageId] {&self.message_ids}
    pub fn rendered_utf8_bytes(&self) -> usize {self.rendered_utf8_bytes}
}
