#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct McpServerName(String);

impl McpServerName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for McpServerName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for McpServerName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for McpServerName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
