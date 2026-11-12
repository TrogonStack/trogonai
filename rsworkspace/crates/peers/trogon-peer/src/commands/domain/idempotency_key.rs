#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyKey(String);
impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, super::PeerValueError> {
        Ok(Self(super::peer_value_error::parse_token(value.into())?))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
