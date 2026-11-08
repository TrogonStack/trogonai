#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid model name")]
pub struct ModelNameError;

impl ModelName {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelNameError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(ModelNameError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
