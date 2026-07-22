use std::fmt;
use std::str::FromStr;

use super::nonblank::{NonBlankViolation, validate_nonblank};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("model id '{raw}' is invalid: {violation}")]
pub struct ModelIdError {
    raw: String,
    violation: NonBlankViolation,
}

impl ModelId {
    pub fn parse(raw: &str) -> Result<Self, ModelIdError> {
        validate_nonblank(raw).map_err(|violation| ModelIdError {
            raw: raw.to_string(),
            violation,
        })?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ModelId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ModelId {
    type Err = ModelIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
