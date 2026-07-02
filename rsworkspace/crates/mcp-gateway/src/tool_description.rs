use std::fmt;

/// Free-text description of an MCP tool. This is the primary attack surface
/// for tool poisoning, hidden instructions, and prompt injection: MCP clients
/// typically inline tool descriptions into the model context verbatim.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolDescription(String);

impl ToolDescription {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ToolDescription {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_empty_description() {
        assert_eq!(ToolDescription::new("").as_str(), "");
    }

    #[test]
    fn round_trips_text() {
        assert_eq!(ToolDescription::new("Search the web").as_str(), "Search the web");
    }
}
