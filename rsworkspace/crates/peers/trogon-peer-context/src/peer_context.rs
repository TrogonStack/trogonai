use trogonai_proto::peers::v1alpha1::{PeerBatch, PreparedMessage};

use crate::PeerContextError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerContext(String);

impl PeerContext {
    pub fn render(_batch: &PeerBatch, _messages: &[PreparedMessage]) -> Result<Self, PeerContextError> {
        Err(PeerContextError::ContractViolation)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
