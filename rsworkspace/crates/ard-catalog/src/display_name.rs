//! Human-readable ARD display name value object.

use std::sync::Arc;

/// Error returned when [`DisplayName`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DisplayNameError {
    #[error("displayName must not be empty")]
    Empty,
}

/// Validated non-empty ARD display name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DisplayName(Arc<str>);

impl DisplayName {
    pub fn new(s: impl AsRef<str>) -> Result<Self, DisplayNameError> {
        let s = s.as_ref();
        if s.trim().is_empty() {
            return Err(DisplayNameError::Empty);
        }
        Ok(Self(Arc::from(s)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DisplayName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests;
