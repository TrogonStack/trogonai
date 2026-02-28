//! NATS-safe session ID value object.
//!
//! Session IDs are embedded as a single NATS subject token: `{prefix}.{session_id}.agent.*`.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! ASCII only (recommended), rejecting `.` `*` `>` and whitespace (forbidden). Validity is
//! guaranteed at construction.
//!
//! TODO: Consider extracting to `trogon-nats` as a generic `NatsSubject` (or `NatsToken`) type
//! so prefix, session_id, and other subject tokens share the same validation.

use crate::subject_token_violation::SubjectTokenViolation;

const MAX_SESSION_ID_LENGTH: usize = 128;

/// Error returned when [`AcpSessionId`] validation fails.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionIdError(pub SubjectTokenViolation);

impl std::fmt::Display for SessionIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SubjectTokenViolation::Empty => write!(f, "session_id must not be empty"),
            SubjectTokenViolation::InvalidCharacter(ch) => {
                write!(f, "session_id contains invalid character: {:?}", ch)
            }
            SubjectTokenViolation::TooLong(len) => {
                write!(f, "session_id is too long: {} characters (max 128)", len)
            }
        }
    }
}

impl std::error::Error for SessionIdError {}

/// NATS-safe session ID. Guarantees validity at construction—invalid instances are unrepresentable.
///
/// Follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
/// ASCII only; rejects `.`, `*`, `>`, and whitespace. Max 128 characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AcpSessionId(std::sync::Arc<str>);

impl AcpSessionId {
    pub fn new(s: impl AsRef<str>) -> Result<Self, SessionIdError> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(SessionIdError(SubjectTokenViolation::Empty));
        }
        let mut char_count = 0;
        for ch in s.chars() {
            char_count += 1;
            if char_count > MAX_SESSION_ID_LENGTH {
                return Err(SessionIdError(SubjectTokenViolation::TooLong(char_count)));
            }
            if !ch.is_ascii() {
                return Err(SessionIdError(SubjectTokenViolation::InvalidCharacter(ch)));
            }
            if ch == '.' || ch == '*' || ch == '>' || ch.is_whitespace() {
                return Err(SessionIdError(SubjectTokenViolation::InvalidCharacter(ch)));
            }
        }
        Ok(Self(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
mod tests {
    use super::*;

    #[test]
    fn acp_session_id_too_long_returns_err() {
        let long_id = "a".repeat(129);
        let err = AcpSessionId::new(&long_id).err().unwrap();
        assert_eq!(err, SessionIdError(SubjectTokenViolation::TooLong(129)));

        assert!(AcpSessionId::new("a".repeat(128).as_str()).is_ok());
    }

    #[test]
    fn acp_session_id_rejects_dots() {
        let err = AcpSessionId::new("session.id").err().unwrap();
        assert_eq!(
            err,
            SessionIdError(SubjectTokenViolation::InvalidCharacter('.'))
        );
    }

    #[test]
    fn acp_session_id_rejects_wildcards() {
        assert!(AcpSessionId::new("session*").is_err());
        assert!(AcpSessionId::new("session>").is_err());
        assert!(AcpSessionId::new(">").is_err());
    }

    #[test]
    fn acp_session_id_rejects_non_ascii() {
        let err = AcpSessionId::new("séssion").err().unwrap();
        assert_eq!(
            err,
            SessionIdError(SubjectTokenViolation::InvalidCharacter('é'))
        );
    }

    #[test]
    fn acp_session_id_valid() {
        assert!(AcpSessionId::new("valid-session-123").is_ok());
        assert!(AcpSessionId::new("a").is_ok());
    }

    #[test]
    fn acp_session_id_empty_returns_err() {
        let err = AcpSessionId::new("").err().unwrap();
        assert_eq!(err, SessionIdError(SubjectTokenViolation::Empty));
    }

    #[test]
    fn acp_session_id_try_from_session_id() {
        use agent_client_protocol::SessionId;

        let valid = SessionId::from("valid-session");
        assert!(AcpSessionId::try_from(&valid).is_ok());
        assert_eq!(
            AcpSessionId::try_from(&valid).unwrap().as_str(),
            "valid-session"
        );

        let invalid = SessionId::from("invalid.session");
        assert!(AcpSessionId::try_from(&invalid).is_err());
    }

    #[test]
    fn acp_session_id_display_and_deref() {
        let id = AcpSessionId::new("my-session").unwrap();
        assert_eq!(format!("{}", id), "my-session");
        assert_eq!(id.len(), 10);
        assert!(id.starts_with("my"));
    }

    #[test]
    fn session_id_error_display() {
        assert_eq!(
            format!("{}", SessionIdError(SubjectTokenViolation::Empty)),
            "session_id must not be empty"
        );
        assert_eq!(
            format!(
                "{}",
                SessionIdError(SubjectTokenViolation::InvalidCharacter('.'))
            ),
            "session_id contains invalid character: '.'"
        );
        assert_eq!(
            format!("{}", SessionIdError(SubjectTokenViolation::TooLong(129))),
            "session_id is too long: 129 characters (max 128)"
        );
    }
}
