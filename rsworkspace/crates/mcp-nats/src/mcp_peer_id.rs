use trogon_nats::{NatsToken, SubjectTokenViolation};

#[derive(Debug, Clone, PartialEq)]
pub struct McpPeerIdError(pub SubjectTokenViolation);

impl std::fmt::Display for McpPeerIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SubjectTokenViolation::Empty => write!(f, "mcp_peer_id must not be empty"),
            SubjectTokenViolation::InvalidCharacter(ch) => {
                write!(f, "mcp_peer_id contains invalid character: {:?}", ch)
            }
            SubjectTokenViolation::TooLong(len) => {
                write!(f, "mcp_peer_id is too long: {} characters (max 128)", len)
            }
        }
    }
}

impl std::error::Error for McpPeerIdError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct McpPeerId(NatsToken);

impl McpPeerId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, McpPeerIdError> {
        NatsToken::new(s).map(Self).map_err(McpPeerIdError)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for McpPeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests;
