use std::fmt;
use std::str::FromStr;

const MAX_LENGTH: usize = 256;

/// Identifier of the agent a kill switch decision applies to.
///
/// Doubles as the decider stream id and the projection's KV key, so its
/// constructor rejects anything that would be unsafe in either role (empty,
/// oversized, or padded with whitespace that would silently change the key).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentIdViolation {
    #[error("must not be empty")]
    Empty,
    #[error("must be at most {max} characters, got {actual}")]
    TooLong { max: usize, actual: usize },
    #[error("must not have leading or trailing whitespace")]
    SurroundingWhitespace,
}

#[derive(Debug, thiserror::Error)]
#[error("agent id '{raw}' is invalid: {violation}")]
pub struct AgentIdError {
    raw: String,
    violation: AgentIdViolation,
}

impl AgentId {
    pub fn parse(raw: &str) -> Result<Self, AgentIdError> {
        if raw.is_empty() {
            return Err(AgentIdError::new(raw, AgentIdViolation::Empty));
        }

        if raw.trim() != raw {
            return Err(AgentIdError::new(raw, AgentIdViolation::SurroundingWhitespace));
        }

        let length = raw.chars().count();
        if length > MAX_LENGTH {
            return Err(AgentIdError::new(
                raw,
                AgentIdViolation::TooLong {
                    max: MAX_LENGTH,
                    actual: length,
                },
            ));
        }

        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for AgentId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for AgentId {
    type Err = AgentIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl AgentIdError {
    fn new(raw: &str, violation: AgentIdViolation) -> Self {
        Self {
            raw: raw.to_string(),
            violation,
        }
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
