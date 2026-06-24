//! NATS-safe session ID value object.
//!
//! Session IDs are embedded as a single NATS subject token: `{prefix}.{session_id}.agent.*`.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! ASCII only (recommended), rejecting `.` `*` `>` and whitespace (forbidden). Validity is
//! guaranteed at construction.

use trogon_nats::NatsToken;
use trogon_nats::SubjectTokenViolation;

/// Error returned when [`AcpSessionId`] validation fails.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SessionIdError {
    #[error("session_id must not be empty")]
    Empty,
    #[error("session_id contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("session_id is too long: {0} characters (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolation> for SessionIdError {
    fn from(v: SubjectTokenViolation) -> Self {
        match v {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// NATS-safe session ID. Guarantees validity at construction—invalid instances are unrepresentable.
///
/// Follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
/// ASCII only; rejects `.`, `*`, `>`, and whitespace. Max 128 characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AcpSessionId(NatsToken);

impl AcpSessionId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, SessionIdError> {
        NatsToken::new(s).map(Self).map_err(Into::into)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for AcpSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for AcpSessionId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<&agent_client_protocol::SessionId> for AcpSessionId {
    type Error = SessionIdError;

    fn try_from(session_id: &agent_client_protocol::SessionId) -> Result<Self, Self::Error> {
        AcpSessionId::new(session_id.to_string().as_str())
    }
}

#[cfg(test)]
mod tests;
