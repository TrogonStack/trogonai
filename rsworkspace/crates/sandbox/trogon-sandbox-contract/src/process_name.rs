use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ProcessNameError;

/// A caller-chosen name for a detached process, unique per lease.
///
/// Starting a second command under a name already held by a process that is still running returns that process's
/// execution id instead of starting another. Once the process has exited the name is free again.
///
/// A name is 1 to [`ProcessName::MAX_LEN`] characters of `[A-Za-z0-9._-]`. Construction, parsing, and deserialization
/// all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProcessName(String);

impl ProcessName {
    /// Longest name accepted, in bytes.
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, ProcessNameError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(ProcessNameError::Length { max: Self::MAX_LEN });
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(ProcessNameError::Characters);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProcessName {
    type Error = ProcessNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ProcessName {
    type Err = ProcessNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ProcessName> for String {
    fn from(value: ProcessName) -> Self {
        value.0
    }
}

impl fmt::Display for ProcessName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
