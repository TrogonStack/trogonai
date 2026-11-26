use super::PeerValueError;
use super::peer_value_error::parse_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDisplayName(String);

impl PeerDisplayName {
    pub fn new(value: impl Into<String>) -> Result<Self, PeerValueError> {
        parse_text(value.into(), 80).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
