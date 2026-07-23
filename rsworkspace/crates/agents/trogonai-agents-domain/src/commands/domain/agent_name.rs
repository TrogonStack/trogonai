use std::fmt;
use std::str::FromStr;

use super::nonblank::{NonBlankViolation, validate_nonblank};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("agent name '{raw}' is invalid: {violation}")]
pub struct AgentNameError {
    raw: String,
    violation: NonBlankViolation,
}

impl AgentName {
    pub fn parse(raw: &str) -> Result<Self, AgentNameError> {
        validate_nonblank(raw).map_err(|violation| AgentNameError {
            raw: raw.to_string(),
            violation,
        })?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for AgentName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for AgentName {
    type Err = AgentNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
