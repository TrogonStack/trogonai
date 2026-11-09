#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct McpToolName(String);

impl McpToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for McpToolName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for McpToolName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for McpToolName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
