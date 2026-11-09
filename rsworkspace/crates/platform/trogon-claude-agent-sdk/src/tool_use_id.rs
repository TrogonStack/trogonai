#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolUseId(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid tool use id")]
pub struct ToolUseIdError;

impl ToolUseId {
    pub fn new(value: impl Into<String>) -> Result<Self, ToolUseIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(ToolUseIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolUseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
