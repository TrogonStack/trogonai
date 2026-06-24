use trogon_nats::{NatsToken, SubjectTokenViolation};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum McpPeerIdError {
    #[error("mcp_peer_id must not be empty")]
    Empty,
    #[error("mcp_peer_id contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("mcp_peer_id is too long: {0} characters (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for McpPeerIdError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct McpPeerId(NatsToken);

impl McpPeerId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, McpPeerIdError> {
        NatsToken::new(s).map(Self).map_err(McpPeerIdError::from)
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
