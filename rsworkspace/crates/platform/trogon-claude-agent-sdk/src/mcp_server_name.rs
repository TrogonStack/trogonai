#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct McpServerName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid MCP server name")]
pub struct McpServerNameError;

impl McpServerName {
    pub fn new(value: impl Into<String>) -> Result<Self, McpServerNameError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(McpServerNameError);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for McpServerName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
