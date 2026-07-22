//! NATS-safe A2A prefix value object.
//!
//! The prefix is embedded in every NATS subject the binding publishes:
//! `{prefix}.agents.{agent_id}.message.send`, `{prefix}.tasks.{task_id}.events.{req_id}`, etc.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! rejects `*`, `>`, whitespace; allows dotted namespaces (e.g. `my.multi.part`) but rejects
//! malformed dots (consecutive, leading, trailing). Max 128 bytes. Validity is guaranteed at
//! construction.

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolation;

/// Error returned when [`A2aPrefix`] validation fails.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum A2aPrefixError {
    #[error("a2a_prefix must not be empty")]
    Empty,
    #[error("a2a_prefix contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("a2a_prefix is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for A2aPrefixError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// NATS-safe A2A prefix. Guarantees validity at construction — invalid instances are unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct A2aPrefix(DottedNatsToken);

impl A2aPrefix {
    pub fn new(s: impl Into<String>) -> Result<Self, A2aPrefixError> {
        let s = s.into();
        DottedNatsToken::new(s).map(Self).map_err(A2aPrefixError::from)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for A2aPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

#[cfg(test)]
mod tests;
