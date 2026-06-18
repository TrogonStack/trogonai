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
mod tests {
    use super::*;

    #[test]
    fn a2a_agent_id_valid() {
        assert!(A2aAgentId::new("support-bot").is_ok());
        assert!(A2aAgentId::new("a").is_ok());
        assert_eq!(A2aAgentId::new("ok").unwrap().as_str(), "ok");
    }

    #[test]
    fn a2a_agent_id_rejects_dots_wildcards_whitespace() {
        assert!(A2aAgentId::new("a.b").is_err());
        assert!(A2aAgentId::new("a*").is_err());
        assert!(A2aAgentId::new("a>").is_err());
        assert!(A2aAgentId::new("a b").is_err());
    }

    #[test]
    fn a2a_agent_id_too_long() {
        let long_id = "a".repeat(129);
        assert!(A2aAgentId::new(&long_id).is_err());
        assert!(A2aAgentId::new("a".repeat(128).as_str()).is_ok());
    }

    #[test]
    fn a2a_agent_id_empty_returns_err() {
        assert_eq!(A2aAgentId::new("").err().unwrap(), AgentIdError::Empty);
    }

    #[test]
    fn a2a_agent_id_display_and_deref() {
        let id = A2aAgentId::new("my-agent").unwrap();
        assert_eq!(format!("{id}"), "my-agent");
        assert_eq!(id.len(), 8);
        assert!(id.starts_with("my"));
    }

    #[test]
    fn a2a_agent_id_from_str_parses_valid_and_rejects_invalid() {
        use std::str::FromStr;
        let id: A2aAgentId = "from-str".parse().unwrap();
        assert_eq!(id.as_str(), "from-str");
        assert!(A2aAgentId::from_str("bad.dot").is_err());
    }

    #[test]
    fn agent_id_error_display() {
        assert_eq!(AgentIdError::Empty.to_string(), "agent_id must not be empty");
        assert_eq!(
            AgentIdError::InvalidCharacter('.').to_string(),
            "agent_id contains invalid character: '.'"
        );
        assert_eq!(
            AgentIdError::TooLong(129).to_string(),
            "agent_id is too long: 129 characters (max 128)"
        );
    }
}
