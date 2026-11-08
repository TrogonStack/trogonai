use crate::constants::MAX_PROMPT_ID_BYTES;
use trogon_peer::{NativeSessionId, PeerValueError};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptId(NativeSessionId);
impl PromptId {
    pub fn new(value: String) -> Result<Self, PeerValueError> {
        if value.len() > MAX_PROMPT_ID_BYTES {
            return Err(PeerValueError::TooLong {
                maximum: MAX_PROMPT_ID_BYTES,
            });
        }
        NativeSessionId::new(value).map(Self)
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
