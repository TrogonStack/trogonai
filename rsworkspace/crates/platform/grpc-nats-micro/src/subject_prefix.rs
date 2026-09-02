//! The NATS subject namespace one service's endpoints are derived under
//! (ADR 0016 §2).

use trogon_nats::{DottedNatsToken, SubjectTokenViolationError};

use crate::subject_prefix_input::SubjectPrefixInput;

/// Why a [`SubjectPrefix`] could not be constructed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SubjectPrefixError {
    #[error("subject prefix must not be empty")]
    Empty,
    #[error("subject prefix contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("subject prefix is too long: {0} bytes")]
    TooLong(usize),
}

impl From<SubjectTokenViolationError> for SubjectPrefixError {
    fn from(violation: SubjectTokenViolationError) -> Self {
        match violation {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// The dotted namespace every endpoint subject of one service is derived under.
///
/// Dotted, so a deployment can namespace by domain and binding version
/// (`echo.v1`). Wildcards and malformed dots are rejected here rather than at
/// registration, because the subject this prefix feeds is a concrete address.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubjectPrefix(DottedNatsToken);

impl SubjectPrefix {
    pub fn from_input(input: &SubjectPrefixInput) -> Result<Self, SubjectPrefixError> {
        DottedNatsToken::new(input.as_str()).map(Self).map_err(Into::into)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for SubjectPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
