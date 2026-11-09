#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolRule(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid tool rule")]
pub struct ToolRuleError;

impl ToolRule {
    pub fn new(value: impl Into<String>) -> Result<Self, ToolRuleError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) || value.contains(',') {
            return Err(ToolRuleError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
