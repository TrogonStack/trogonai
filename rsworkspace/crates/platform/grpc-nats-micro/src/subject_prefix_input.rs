//! The configured subject namespace exactly as a deployment supplied it
//! (ADR 0016 §2).

/// Untrusted subject prefix text. Carries no guarantee that the value is a
/// dotted run of legal subject tokens, or that it is free of the wildcards a
/// concrete address may not contain; [`crate::SubjectPrefix::from_input`] is
/// the single conversion into the domain value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectPrefixInput(Box<str>);

impl SubjectPrefixInput {
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SubjectPrefixInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
