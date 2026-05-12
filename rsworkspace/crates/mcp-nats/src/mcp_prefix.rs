use trogon_nats::{DottedNatsToken, SubjectTokenViolation};

#[derive(Debug, Clone, PartialEq)]
pub struct McpPrefixError(pub SubjectTokenViolation);

impl std::fmt::Display for McpPrefixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SubjectTokenViolation::Empty => write!(f, "mcp_prefix must not be empty"),
            SubjectTokenViolation::InvalidCharacter(ch) => {
                write!(f, "mcp_prefix contains invalid character: {:?}", ch)
            }
            SubjectTokenViolation::TooLong(len) => {
                write!(f, "mcp_prefix is too long: {} bytes (max 128)", len)
            }
        }
    }
}

impl std::error::Error for McpPrefixError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct McpPrefix(DottedNatsToken);

impl McpPrefix {
    pub fn new(s: impl AsRef<str>) -> Result<Self, McpPrefixError> {
        DottedNatsToken::new(s).map(Self).map_err(McpPrefixError)
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
