use trogon_nats::{DottedNatsToken, SubjectTokenViolation};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum McpPrefixError {
    #[error("mcp_prefix must not be empty")]
    Empty,
    #[error("mcp_prefix contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("mcp_prefix is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for McpPrefixError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct McpPrefix(DottedNatsToken);

impl McpPrefix {
    pub fn new(s: impl AsRef<str>) -> Result<Self, McpPrefixError> {
        DottedNatsToken::new(s).map(Self).map_err(McpPrefixError::from)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for McpPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_accepts_dotted_namespace() {
        assert_eq!(McpPrefix::new("mcp").unwrap().as_str(), "mcp");
        assert_eq!(McpPrefix::new("tenant.mcp").unwrap().as_str(), "tenant.mcp");
    }

    #[test]
    fn prefix_rejects_invalid_subject_tokens() {
        assert!(McpPrefix::new("").is_err());
        assert!(McpPrefix::new("mcp.*").is_err());
        assert!(McpPrefix::new("mcp.>").is_err());
        assert!(McpPrefix::new("mcp prefix").is_err());
        assert!(McpPrefix::new("mcp..tenant").is_err());
    }

    #[test]
    fn prefix_error_display_covers_validation_failures() {
        assert_eq!(
            McpPrefix::new("").unwrap_err().to_string(),
            "mcp_prefix must not be empty"
        );
        assert_eq!(
            McpPrefix::new("mcp.*").unwrap_err().to_string(),
            "mcp_prefix contains invalid character: '*'"
        );
        assert_eq!(
            McpPrefix::new("a".repeat(129)).unwrap_err().to_string(),
            "mcp_prefix is too long: 129 bytes (max 128)"
        );
    }
}
