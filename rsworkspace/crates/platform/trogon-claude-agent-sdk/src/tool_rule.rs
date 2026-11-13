#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolRule(String);

impl ToolRule {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ToolRule {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ToolRule {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for ToolRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
