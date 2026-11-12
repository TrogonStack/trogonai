#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolName(String);

impl ToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ToolName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ToolName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
