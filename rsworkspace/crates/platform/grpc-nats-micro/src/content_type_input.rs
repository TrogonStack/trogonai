//! The `Content-Type` header exactly as a caller sent it (ADR 0016 §4).

/// Untrusted `Content-Type` header text. Carries no guarantee that the value
/// names an encoding this binding speaks; [`crate::ContentType::from_input`]
/// is the single conversion into the domain value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentTypeInput(Box<str>);

impl ContentTypeInput {
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentTypeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
