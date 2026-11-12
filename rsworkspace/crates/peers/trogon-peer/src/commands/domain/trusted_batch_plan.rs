use super::{MessageId, PeerValueError};
#[derive(Debug, Clone)]
pub struct TrustedBatchPlan {
    pub(crate) message_ids: Vec<MessageId>,
    pub(crate) rendered_utf8_bytes: usize,
}
impl TrustedBatchPlan {
    pub fn from_renderer(message_ids: Vec<MessageId>, rendered_utf8_bytes: usize) -> Result<Self, PeerValueError> {
        if message_ids.is_empty()
            || message_ids.len() > 4
            || !(1..=8192).contains(&rendered_utf8_bytes)
            || message_ids
                .iter()
                .enumerate()
                .any(|(i, id)| message_ids[..i].contains(id))
        {
            return Err(PeerValueError::InvalidBatchPlan);
        }
        Ok(Self {
            message_ids,
            rendered_utf8_bytes,
        })
    }
    pub fn message_ids(&self) -> &[MessageId] {
        &self.message_ids
    }
    pub fn rendered_utf8_bytes(&self) -> usize {
        self.rendered_utf8_bytes
    }
}
