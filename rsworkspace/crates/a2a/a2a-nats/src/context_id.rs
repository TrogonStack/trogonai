//! NATS-safe context identifier value object.
//!
//! Per A2A spec, a Context groups related Tasks and Messages across multi-turn interactions.
//! Used for `Task.context_id`, `Message.context_id`, and update events. Not currently embedded
//! in NATS subjects, but we model it as a value object for consistency and so we can later
//! shape per-context audit subjects (e.g. `a2a.audit.context.{context_id}.>`).

use trogon_nats::NatsToken;
use trogon_nats::SubjectTokenViolation;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ContextIdError {
    #[error("context_id must not be empty")]
    Empty,
    #[error("context_id contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("context_id is too long: {0} characters (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for ContextIdError {
    fn from(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct A2aContextId(NatsToken);

impl A2aContextId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, ContextIdError> {
        NatsToken::new(s).map(Self).map_err(ContextIdError::from)
    }

    #[allow(clippy::expect_used)]
    pub fn generate() -> Self {
        let id = uuid::Uuid::now_v7().simple().to_string();
        Self(NatsToken::new(id).expect("uuid v7 simple form is a valid NATS token"))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for A2aContextId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for A2aContextId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests;
