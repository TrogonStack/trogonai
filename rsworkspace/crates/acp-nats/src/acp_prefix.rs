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
mod tests {
    use super::*;

    #[test]
    fn acp_prefix_new_valid() {
        let p = AcpPrefix::new("acp").unwrap();
        assert_eq!(p.as_str(), "acp");
        assert_eq!(AcpPrefix::new("my.multi.part").unwrap().as_str(), "my.multi.part");
    }

    #[test]
    fn acp_prefix_new_invalid_returns_err() {
        assert!(AcpPrefix::new("").is_err());
        assert!(AcpPrefix::new("acp.*").is_err());
        assert!(AcpPrefix::new("acp.>").is_err());
        assert!(AcpPrefix::new("acp prefix").is_err());
        assert!(AcpPrefix::new("acp\t").is_err());
        assert!(AcpPrefix::new("acp\n").is_err());
        assert!(AcpPrefix::new("acp..foo").is_err());
        assert!(AcpPrefix::new(".acp").is_err());
        assert!(AcpPrefix::new("acp.").is_err());
        assert!(AcpPrefix::new("a".repeat(129)).is_err());
    }

    #[test]
    fn acp_prefix_new_validates_direct() {
        assert!(AcpPrefix::new("acp").is_ok());
        assert!(AcpPrefix::new("a").is_ok());
        assert!(AcpPrefix::new("my.multi.part").is_ok());
        assert!(AcpPrefix::new("a".repeat(128)).is_ok());
        assert!(matches!(AcpPrefix::new(""), Err(AcpPrefixError::Empty)));
        assert!(matches!(
            AcpPrefix::new("a".repeat(129)),
            Err(AcpPrefixError::TooLong(129))
        ));
    }

    #[test]
    fn acp_prefix_error_display() {
        assert_eq!(format!("{}", AcpPrefixError::Empty), "acp_prefix must not be empty");
        assert_eq!(
            format!("{}", AcpPrefixError::InvalidCharacter('*')),
            "acp_prefix contains invalid character: '*'"
        );
        assert_eq!(
            format!("{}", AcpPrefixError::TooLong(200)),
            "acp_prefix is too long: 200 bytes (max 128)"
        );
    }
}
