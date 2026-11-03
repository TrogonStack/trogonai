use crate::RoutingTokenError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConnectionId(String);

impl PeerConnectionId {
    pub fn new(value: impl Into<String>) -> Result<Self, RoutingTokenError> {
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
