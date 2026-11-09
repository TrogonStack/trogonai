use crate::ToolName;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BuiltinToolName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BuiltinToolNameError {
    #[error("a built-in tool name cannot be empty")]
    Empty,
    #[error("a built-in tool name cannot contain whitespace, controls or list separators")]
    InvalidCharacter,
    #[error("the default tool preset must be selected through ToolSet::CliDefault")]
    ReservedPreset,
}

impl BuiltinToolName {
    pub fn new(value: impl Into<String>) -> Result<Self, BuiltinToolNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(BuiltinToolNameError::Empty);
        }
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control() || character == ',')
        {
            return Err(BuiltinToolNameError::InvalidCharacter);
        }
        if value == "default" {
            return Err(BuiltinToolNameError::ReservedPreset);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<ToolName> for BuiltinToolName {
    type Error = BuiltinToolNameError;

    fn try_from(value: ToolName) -> Result<Self, Self::Error> {
        Self::new(value.as_str())
    }
}
