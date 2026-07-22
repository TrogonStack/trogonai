use std::fmt;
use std::str::FromStr;

use super::nonblank::{NonBlankViolation, validate_nonblank};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("runtime id '{raw}' is invalid: {violation}")]
pub struct RuntimeIdError {
    raw: String,
    violation: NonBlankViolation,
}

impl RuntimeId {
    pub fn parse(raw: &str) -> Result<Self, RuntimeIdError> {
        validate_nonblank(raw).map_err(|violation| RuntimeIdError {
            raw: raw.to_string(),
            violation,
        })?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RuntimeId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for RuntimeId {
    type Err = RuntimeIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
