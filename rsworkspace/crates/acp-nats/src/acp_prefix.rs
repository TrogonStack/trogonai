//! NATS-safe ACP prefix value object.
//!
//! The prefix is embedded in every NATS subject the bridge publishes:
//! `{prefix}.agent.*`, `{prefix}.session.*`, etc.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! rejects `*`, `>`, whitespace; allows dotted namespaces (e.g. `my.multi.part`) but rejects
//! malformed dots (consecutive, leading, trailing). Max 128 bytes. Validity is guaranteed at
//! construction.

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolation;

/// Error returned when [`AcpPrefix`] validation fails.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AcpPrefixError {
    #[error("acp_prefix must not be empty")]
    Empty,
    #[error("acp_prefix contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("acp_prefix is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for AcpPrefixError {
    fn from(v: SubjectTokenViolation) -> Self {
        match v {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// NATS-safe ACP prefix. Guarantees validity at construction—invalid instances are unrepresentable.
#[derive(Clone, Debug)]
pub struct AcpPrefix(DottedNatsToken);

impl AcpPrefix {
    pub fn new(s: impl Into<String>) -> Result<Self, AcpPrefixError> {
        let s = s.into();
        DottedNatsToken::new(s).map(Self).map_err(Into::into)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[cfg(test)]
mod tests;
