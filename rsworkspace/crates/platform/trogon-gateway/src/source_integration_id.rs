use std::fmt;

use trogon_nats::{NatsToken, SubjectTokenViolationError};

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

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SourceIntegrationIdError {
    #[error("source integration id must not be empty")]
    Empty,
    #[error("source integration id contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("source integration id exceeds maximum length: {0}")]
    TooLong(usize),
}

impl SourceIntegrationIdError {
    fn from_subject_token_violation(violation: SubjectTokenViolationError) -> Self {
        match violation {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(length) => Self::TooLong(length),
        }
    }
}

#[cfg(test)]
mod tests;
