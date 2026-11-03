use crate::constants::MAX_PROMPT_ID_BYTES;
use trogon_peer::PeerValueError;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptId(String);
impl PromptId {
    pub fn new(value: String) -> Result<Self, PeerValueError> {
        if value.is_empty() || value.len() > MAX_PROMPT_ID_BYTES || value.chars().any(char::is_control) {
            return Err(PeerValueError::InvalidBatchPlan);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
