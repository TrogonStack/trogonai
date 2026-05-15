use std::fmt;

use trogon_nats::{NatsToken, SubjectTokenViolation};

const MAX_INTEGRATION_ID_LEN: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIntegrationId(NatsToken);

impl SourceIntegrationId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SourceIntegrationIdError> {
        let value = value.as_ref();

        let char_count = value.chars().count();
        if char_count > MAX_INTEGRATION_ID_LEN {
            return Err(SourceIntegrationIdError::TooLong(char_count));
        }

        let token = NatsToken::new(value).map_err(SourceIntegrationIdError::from_subject_token_violation)?;

        for ch in token.as_str().chars() {
            if !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_' {
                return Err(SourceIntegrationIdError::InvalidCharacter(ch));
            }
        }

        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn stream_name_suffix(&self) -> String {
        self.as_str().chars().map(|ch| ch.to_ascii_uppercase()).collect()
    }
}

impl fmt::Display for SourceIntegrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceIntegrationIdError {
    Empty,
    InvalidCharacter(char),
    TooLong(usize),
}

impl SourceIntegrationIdError {
    fn from_subject_token_violation(violation: SubjectTokenViolation) -> Self {
        match violation {
            SubjectTokenViolation::Empty => Self::Empty,
            SubjectTokenViolation::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolation::TooLong(length) => Self::TooLong(length),
        }
    }
}

impl fmt::Display for SourceIntegrationIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("source integration id must not be empty"),
            Self::InvalidCharacter(ch) => write!(f, "source integration id contains invalid character '{ch}'"),
            Self::TooLong(length) => write!(f, "source integration id exceeds maximum length: {length}"),
        }
    }
}

impl std::error::Error for SourceIntegrationIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_route_safe_ids() {
        let id = SourceIntegrationId::new("acme-main_1").unwrap();

        assert_eq!(id.as_str(), "acme-main_1");
        assert_eq!(id.stream_name_suffix(), "ACME-MAIN_1");
    }

    #[test]
    fn stream_name_suffix_preserves_hyphen_and_underscore_distinction() {
        let hyphen = SourceIntegrationId::new("acme-main").unwrap();
        let underscore = SourceIntegrationId::new("acme_main").unwrap();

        assert_eq!(hyphen.stream_name_suffix(), "ACME-MAIN");
        assert_eq!(underscore.stream_name_suffix(), "ACME_MAIN");
    }

    #[test]
    fn rejects_path_separators() {
        assert_eq!(
            SourceIntegrationId::new("acme/main"),
            Err(SourceIntegrationIdError::InvalidCharacter('/'))
        );
    }

    #[test]
    fn rejects_nats_unsafe_characters() {
        assert_eq!(
            SourceIntegrationId::new("acme.main"),
            Err(SourceIntegrationIdError::InvalidCharacter('.'))
        );
        assert_eq!(
            SourceIntegrationId::new("acme*main"),
            Err(SourceIntegrationIdError::InvalidCharacter('*'))
        );
    }

    #[test]
    fn rejects_too_long_ids_with_actual_length() {
        let value = "a".repeat(200);
        let err = SourceIntegrationId::new(&value).unwrap_err();

        assert_eq!(err, SourceIntegrationIdError::TooLong(200));
        assert_eq!(err.to_string(), "source integration id exceeds maximum length: 200");
    }
}
