use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

/// Stable identifier of the actor a command is executed on behalf of.
///
/// Stable is the operative word: an authorizer that grants on an identifier
/// that can be reassigned grants to whoever holds it next. What makes an
/// identifier stable is the issuing boundary's problem, not this type's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalId(String);

impl PrincipalId {
    /// Creates a principal identifier after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PrincipalIdError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(PrincipalIdError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    pub(crate) fn from_subject_token(token: &str) -> Self {
        Self(token.to_owned())
    }

    /// Returns the identifier as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PrincipalId {
    type Err = PrincipalIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for PrincipalId {
    type Error = PrincipalIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for PrincipalId {
    type Error = PrincipalIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for PrincipalId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for PrincipalId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`PrincipalId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalIdError {
    /// Principal identifiers cannot be empty.
    #[error("principal id cannot be empty")]
    Empty,
    /// Principal identifiers cannot contain control characters.
    #[error("principal id cannot contain control characters")]
    ContainsControlCharacter,
}

#[cfg(test)]
mod tests;
