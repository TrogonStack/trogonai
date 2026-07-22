use std::fmt;
use std::str::FromStr;

use super::nonblank::{NonBlankViolation, validate_nonblank};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParentRef(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("parent reference '{raw}' is invalid: {violation}")]
pub struct ParentRefError {
    raw: String,
    violation: NonBlankViolation,
}

impl ParentRef {
    pub fn parse(raw: &str) -> Result<Self, ParentRefError> {
        validate_nonblank(raw).map_err(|violation| ParentRefError {
            raw: raw.to_string(),
            violation,
        })?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ParentRef {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ParentRef {
    type Err = ParentRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for ParentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
