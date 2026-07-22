//! NATS-safe agent identifier value object.
//!
//! Agent IDs identify a deployed A2A agent and are embedded as a single NATS subject token:
//! `{prefix}.agents.{agent_id}.message.send`. Multiple replicas of an agent share the same
//! agent_id and participate in a NATS queue group on `{prefix}.agents.{agent_id}.>`.

use trogon_nats::NatsToken;
use trogon_nats::SubjectTokenViolation;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AgentIdError {
    #[error("agent_id must not be empty")]
    Empty,
    #[error("agent_id contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("agent_id is too long: {0} characters (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for AgentIdError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct A2aAgentId(NatsToken);

impl A2aAgentId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, AgentIdError> {
        NatsToken::new(s).map(Self).map_err(AgentIdError::from)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for A2aAgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for A2aAgentId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::str::FromStr for A2aAgentId {
    type Err = AgentIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests;
