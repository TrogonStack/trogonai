#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ControlRequestId(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid control request id")]
pub struct ControlRequestIdError;

impl ControlRequestId {
    pub fn new(value: impl Into<String>) -> Result<Self, ControlRequestIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(ControlRequestIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ControlRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
