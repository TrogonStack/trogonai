use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentIdViolation {
    #[error("must not be empty")]
    Empty,
    #[error("must not have leading or trailing whitespace")]
    SurroundingWhitespace,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
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
