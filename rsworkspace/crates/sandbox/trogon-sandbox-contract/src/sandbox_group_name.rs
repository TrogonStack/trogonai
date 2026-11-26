use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::UnixNameError;
use crate::unix_name::validate_unix_name;

/// A Linux group name a worker may create inside a sandbox.
///
/// It matches `^[a-z_][a-z0-9_-]*$` within 32 bytes, the rule `useradd` and `groupadd` accept.
/// Construction, parsing, and deserialization all enforce it, so a caller-controlled string never
/// reaches a worker's account commands unvalidated.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxGroupName(String);

impl SandboxGroupName {
    pub fn new(value: impl Into<String>) -> Result<Self, UnixNameError> {
        let value = value.into();
        validate_unix_name(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SandboxGroupName {
    type Error = UnixNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SandboxGroupName {
    type Err = UnixNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SandboxGroupName> for String {
    fn from(value: SandboxGroupName) -> Self {
        value.0
    }
}

impl fmt::Display for SandboxGroupName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
