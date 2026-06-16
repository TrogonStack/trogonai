//! NATS-safe context identifier value object.
//!
//! Per A2A spec, a Context groups related Tasks and Messages across multi-turn interactions.
//! Used for `Task.context_id`, `Message.context_id`, and update events. Not currently embedded
//! in NATS subjects, but we model it as a value object for consistency and so we can later
//! shape per-context audit subjects (e.g. `a2a.audit.context.{context_id}.>`).

use trogon_nats::NatsToken;
use trogon_nats::SubjectTokenViolation;

#[derive(Debug, Clone, PartialEq)]
pub struct ContextIdError(pub SubjectTokenViolation);

impl std::fmt::Display for ContextIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SubjectTokenViolation::Empty => write!(f, "context_id must not be empty"),
            SubjectTokenViolation::InvalidCharacter(ch) => {
                write!(f, "context_id contains invalid character: {:?}", ch)
            }
            SubjectTokenViolation::TooLong(len) => {
                write!(f, "context_id is too long: {} characters (max 128)", len)
            }
        }
    }
}

impl std::error::Error for ContextIdError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct A2aContextId(NatsToken);

impl A2aContextId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, ContextIdError> {
        NatsToken::new(s).map(Self).map_err(ContextIdError)
    }

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
mod tests {
    use super::*;

    #[test]
    fn a2a_context_id_valid() {
        assert!(A2aContextId::new("ctx-1").is_ok());
    }

    #[test]
    fn a2a_context_id_rejects_invalid() {
        assert!(A2aContextId::new("").is_err());
        assert!(A2aContextId::new("a.b").is_err());
        assert!(A2aContextId::new("a*").is_err());
    }

    #[test]
    fn a2a_context_id_generate_unique() {
        let a = A2aContextId::generate();
        let b = A2aContextId::generate();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn context_id_error_display() {
        assert_eq!(
            format!("{}", ContextIdError(SubjectTokenViolation::Empty)),
            "context_id must not be empty"
        );
        assert_eq!(
            format!("{}", ContextIdError(SubjectTokenViolation::InvalidCharacter('.'))),
            "context_id contains invalid character: '.'"
        );
        assert_eq!(
            format!("{}", ContextIdError(SubjectTokenViolation::TooLong(200))),
            "context_id is too long: 200 characters (max 128)"
        );
    }
}
