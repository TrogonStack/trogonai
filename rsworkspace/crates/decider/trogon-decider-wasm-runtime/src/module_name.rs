use std::borrow::Borrow;
use std::str::FromStr;

/// Stable identity name of a WASM decider module, as reported by its descriptor.
///
/// Module names are stored exactly as provided. Empty names and control
/// characters are rejected, since a module name is a routing and diagnostic
/// key, not free-form text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleName(String);

impl ModuleName {
    /// Creates a module name after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, ModuleNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ModuleNameError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(ModuleNameError::ContainsControlCharacter);
        }
        if value.contains(['@', '/']) {
            return Err(ModuleNameError::ContainsReservedCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the module name as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ModuleName {
    type Err = ModuleNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for ModuleName {
    type Error = ModuleNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ModuleName {
    type Error = ModuleNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for ModuleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for ModuleName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ModuleName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`ModuleName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModuleNameError {
    /// Module names cannot be empty.
    #[error("module name cannot be empty")]
    Empty,
    /// Module names cannot contain control characters.
    #[error("module name cannot contain control characters")]
    ContainsControlCharacter,
    /// Module names cannot contain '@' or '/', which delimit snapshot id segments.
    #[error("module name cannot contain '@' or '/'")]
    ContainsReservedCharacter,
}

#[cfg(test)]
mod tests;
