use trogon_nats::{DottedNatsToken, SubjectTokenViolationError};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum McpPrefixError {
    #[error("mcp_prefix must not be empty")]
    Empty,
    #[error("mcp_prefix contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("mcp_prefix is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolationError> for McpPrefixError {
    fn from(violation: SubjectTokenViolationError) -> Self {
        match violation {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(len) => Self::TooLong(len),
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
mod tests;
