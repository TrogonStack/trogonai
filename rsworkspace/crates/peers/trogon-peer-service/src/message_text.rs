use trogon_peer::PeerValueError;
use trogon_peer_context::MAX_MESSAGE_BYTES;
#[derive(Clone, PartialEq, Eq)]
pub struct MessageText(String);
impl MessageText {
    pub fn new(text: String) -> Result<Self, PeerValueError> {
        if text.is_empty() || text.len() > MAX_MESSAGE_BYTES {
            return Err(PeerValueError::InvalidContent);
        }
        Ok(Self(text))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
