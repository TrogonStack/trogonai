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
mod tests;
