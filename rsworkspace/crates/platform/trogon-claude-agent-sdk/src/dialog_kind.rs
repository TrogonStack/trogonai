#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DialogKind(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid dialog kind")]
pub struct DialogKindError;

impl DialogKind {
    pub fn new(value: impl Into<String>) -> Result<Self, DialogKindError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(DialogKindError);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
