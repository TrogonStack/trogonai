use trogon_peer::{NativeSessionId, PeerValueError};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptId(NativeSessionId);
impl PromptId {
    pub fn new(value: String) -> Result<Self, PeerValueError> {
        NativeSessionId::new(value).map(Self)
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
