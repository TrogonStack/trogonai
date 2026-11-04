use trogonai_proto::peers::v1alpha1::{PeerBatch, PreparedMessage};
#[derive(Clone, PartialEq)]
pub struct PreparedPresentation {
    batch: PeerBatch,
    messages: Vec<PreparedMessage>,
    frame: String,
}
impl PreparedPresentation {
    /// Storage boundary only. The application verifies the complete record before returning any cached payload.
    pub fn from_port(batch: PeerBatch, messages: Vec<PreparedMessage>, frame: String) -> Self {
        Self { batch, messages, frame }
    }
    pub fn batch(&self) -> &PeerBatch {
        &self.batch
    }
    pub fn messages(&self) -> &[PreparedMessage] {
        &self.messages
    }
    pub fn frame(&self) -> &str {
        &self.frame
    }
}
