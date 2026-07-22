use std::borrow::Borrow;
use std::str::FromStr;

/// Stable command type name used to route a command envelope to a decider module.
///
/// Command types are stored exactly as provided by the WIT module descriptor or
/// the caller's command envelope. Empty names and control characters are never
/// valid, since they cannot round-trip through the WIT `string` boundary as a
/// meaningful identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandType(String);

impl CommandType {
    /// Creates a command type after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, CommandTypeError> {
        let value = value.into();
        if value.is_empty() {
            return Err(CommandTypeError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(CommandTypeError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the command type as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommandType {
    type Err = CommandTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for CommandType {
    type Error = CommandTypeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for CommandType {
    type Error = CommandTypeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for CommandType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for CommandType {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for CommandType {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`CommandType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommandTypeError {
    /// Command types cannot be empty.
    #[error("command type cannot be empty")]
    Empty,
    /// Command types cannot contain control characters.
    #[error("command type cannot contain control characters")]
    ContainsControlCharacter,
}

#[cfg(test)]
mod tests;
