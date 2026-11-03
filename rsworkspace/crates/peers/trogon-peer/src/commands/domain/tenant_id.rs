use super::PeerValueError;
use super::peer_value_error::parse_token;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantId(String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Result<Self, PeerValueError> {
        parse_token(value.into()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
