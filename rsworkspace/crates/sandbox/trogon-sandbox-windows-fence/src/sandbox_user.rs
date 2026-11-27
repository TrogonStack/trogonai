use std::fmt;
use std::str::FromStr;

use crate::sandbox_user_error::SandboxUserError;

/// Name of the local account sandboxed commands run as.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SandboxUser(String);

impl SandboxUser {
    pub fn new(value: impl Into<String>) -> Result<Self, SandboxUserError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SandboxUserError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SandboxUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SandboxUser {
    type Err = SandboxUserError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}
