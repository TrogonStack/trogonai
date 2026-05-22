//! NATS-safe A2A prefix value object.
//!
//! The prefix is embedded in every NATS subject the binding publishes:
//! `{prefix}.agent.{agent_id}.message.send`, `{prefix}.task.{task_id}.events.{req_id}`, etc.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! rejects `*`, `>`, whitespace; allows dotted namespaces (e.g. `my.multi.part`) but rejects
//! malformed dots (consecutive, leading, trailing). Max 128 bytes. Validity is guaranteed at
//! construction.

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolation;

/// Error returned when [`A2aPrefix`] validation fails.
#[derive(Debug, Clone, PartialEq)]
pub struct A2aPrefixError(pub SubjectTokenViolation);

impl std::fmt::Display for A2aPrefixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SubjectTokenViolation::Empty => write!(f, "a2a_prefix must not be empty"),
            SubjectTokenViolation::InvalidCharacter(ch) => {
                write!(f, "a2a_prefix contains invalid character: {:?}", ch)
            }
            SubjectTokenViolation::TooLong(len) => {
                write!(f, "a2a_prefix is too long: {} bytes (max 128)", len)
            }
        }
    }
}

impl std::error::Error for A2aPrefixError {}

/// NATS-safe A2A prefix. Guarantees validity at construction — invalid instances are unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct A2aPrefix(DottedNatsToken);

impl A2aPrefix {
    pub fn new(s: impl Into<String>) -> Result<Self, A2aPrefixError> {
        let s = s.into();
        DottedNatsToken::new(s).map(Self).map_err(A2aPrefixError)
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
mod tests {
    use super::*;

    #[test]
    fn a2a_prefix_new_valid() {
        let p = A2aPrefix::new("a2a").unwrap();
        assert_eq!(p.as_str(), "a2a");
        assert_eq!(A2aPrefix::new("my.multi.part").unwrap().as_str(), "my.multi.part");
    }

    #[test]
    fn a2a_prefix_new_invalid_returns_err() {
        assert!(A2aPrefix::new("").is_err());
        assert!(A2aPrefix::new("a2a.*").is_err());
        assert!(A2aPrefix::new("a2a.>").is_err());
        assert!(A2aPrefix::new("a2a prefix").is_err());
        assert!(A2aPrefix::new("a2a\t").is_err());
        assert!(A2aPrefix::new("a2a\n").is_err());
        assert!(A2aPrefix::new("a2a..foo").is_err());
        assert!(A2aPrefix::new(".a2a").is_err());
        assert!(A2aPrefix::new("a2a.").is_err());
        assert!(A2aPrefix::new("a".repeat(129)).is_err());
    }

    #[test]
    fn a2a_prefix_display() {
        let p = A2aPrefix::new("a2a").unwrap();
        assert_eq!(format!("{p}"), "a2a");
    }

    #[test]
    fn a2a_prefix_error_display() {
        assert_eq!(
            format!("{}", A2aPrefixError(SubjectTokenViolation::Empty)),
            "a2a_prefix must not be empty"
        );
        assert_eq!(
            format!("{}", A2aPrefixError(SubjectTokenViolation::InvalidCharacter('*'))),
            "a2a_prefix contains invalid character: '*'"
        );
        assert_eq!(
            format!("{}", A2aPrefixError(SubjectTokenViolation::TooLong(200))),
            "a2a_prefix is too long: 200 bytes (max 128)"
        );
    }
}
