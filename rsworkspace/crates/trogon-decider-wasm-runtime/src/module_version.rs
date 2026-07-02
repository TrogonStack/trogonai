use std::borrow::Borrow;
use std::str::FromStr;

/// Version string of a WASM decider module, as reported by its descriptor.
///
/// Module versions are opaque strings from the runtime's perspective. This
/// crate does not parse or compare them as semantic versions; it only uses
/// them, verbatim, as part of the [`crate::WasmSnapshotId`] identity so a
/// version bump naturally invalidates prior snapshots.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleVersion(String);

impl ModuleVersion {
    /// Creates a module version after rejecting invalid input.
    pub fn new(value: impl Into<String>) -> Result<Self, ModuleVersionError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ModuleVersionError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(ModuleVersionError::ContainsControlCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the module version as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ModuleVersion {
    type Err = ModuleVersionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for ModuleVersion {
    type Error = ModuleVersionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ModuleVersion {
    type Error = ModuleVersionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl std::fmt::Display for ModuleVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for ModuleVersion {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ModuleVersion {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when constructing an invalid [`ModuleVersion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModuleVersionError {
    /// Module versions cannot be empty.
    #[error("module version cannot be empty")]
    Empty,
    /// Module versions cannot contain control characters.
    #[error("module version cannot contain control characters")]
    ContainsControlCharacter,
}

#[cfg(test)]
mod tests;
