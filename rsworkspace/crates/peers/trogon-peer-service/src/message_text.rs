use trogon_peer::PeerValueError;
#[derive(Clone, PartialEq, Eq)]
pub struct MessageText(String);
impl MessageText {
    pub fn new(text: String) -> Result<Self, PeerValueError> {
        if text.is_empty() || text.len() > 2048 {
            return Err(PeerValueError::InvalidContent);
        }
        Ok(Self(text))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
