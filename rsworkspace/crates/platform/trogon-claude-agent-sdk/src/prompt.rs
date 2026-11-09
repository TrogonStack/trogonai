#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Prompt(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid prompt")]
pub struct PromptError;

impl Prompt {
    pub fn new(value: impl Into<String>) -> Result<Self, PromptError> {
        let value = value.into();
        if value.is_empty() || value.contains('\0') {
            return Err(PromptError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
