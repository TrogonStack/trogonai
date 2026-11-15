use crate::RoutingTokenError;
use crate::routing_token::RoutingToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConnectionId(RoutingToken);

impl PeerConnectionId {
    pub fn new(value: impl Into<String>) -> Result<Self, RoutingTokenError> {
        RoutingToken::new(value.into()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
