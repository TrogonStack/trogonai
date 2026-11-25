use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ReadinessNeedleError;

/// The text whose appearance in a process's stdout or stderr marks it ready.
///
/// A needle is 1 to [`ReadinessNeedle::MAX_LEN`] bytes. Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReadinessNeedle(String);

impl ReadinessNeedle {
    /// Longest needle accepted, in bytes.
    pub const MAX_LEN: usize = 4096;

    pub fn new(value: impl Into<String>) -> Result<Self, ReadinessNeedleError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_LEN {
            return Err(ReadinessNeedleError::Length { max: Self::MAX_LEN });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ReadinessNeedle {
    type Error = ReadinessNeedleError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ReadinessNeedle {
    type Err = ReadinessNeedleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ReadinessNeedle> for String {
    fn from(value: ReadinessNeedle) -> Self {
        value.0
    }
}

impl fmt::Display for ReadinessNeedle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
