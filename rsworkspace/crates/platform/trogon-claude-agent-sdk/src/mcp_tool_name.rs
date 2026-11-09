#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct McpToolName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("MCP tool names must contain between 1 and 128 ASCII letters, digits, underscores, hyphens or periods")]
pub struct McpToolNameError;

impl McpToolName {
    pub fn new(value: impl Into<String>) -> Result<Self, McpToolNameError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(McpToolNameError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for McpToolName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
