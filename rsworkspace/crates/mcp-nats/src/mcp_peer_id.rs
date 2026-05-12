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
mod tests {
    use super::*;

    #[test]
    fn peer_id_accepts_single_subject_token() {
        assert_eq!(McpPeerId::new("server-1").unwrap().as_str(), "server-1");
    }

    #[test]
    fn peer_id_rejects_dots_and_wildcards() {
        assert!(McpPeerId::new("server.1").is_err());
        assert!(McpPeerId::new("server*").is_err());
        assert!(McpPeerId::new("server>").is_err());
    }

    #[test]
    fn peer_id_error_display_covers_validation_failures() {
        assert_eq!(
            McpPeerId::new("").unwrap_err().to_string(),
            "mcp_peer_id must not be empty"
        );
        assert_eq!(
            McpPeerId::new("server.1").unwrap_err().to_string(),
            "mcp_peer_id contains invalid character: '.'"
        );
        assert_eq!(
            McpPeerId::new("a".repeat(129)).unwrap_err().to_string(),
            "mcp_peer_id is too long: 129 characters (max 128)"
        );
    }
}
