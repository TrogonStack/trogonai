//! One `rpc` method's name, which becomes both the endpoint subject's final
//! token and the micro endpoint's discovery name (ADR 0016 §2).

use trogon_nats::{NatsToken, SubjectTokenViolationError};

use crate::method_name_input::MethodNameInput;

/// Why a [`MethodName`] could not be constructed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MethodNameError {
    #[error("method name must not be empty")]
    Empty,
    #[error("method name must start with an ASCII letter or underscore, found {0:?}")]
    LeadingCharacter(char),
    #[error("method name contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("method name is too long: {0} characters")]
    TooLong(usize),
}

impl From<SubjectTokenViolationError> for MethodNameError {
    fn from(violation: SubjectTokenViolationError) -> Self {
        match violation {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// A protobuf `rpc` method name that is safe to use as a subject token and as
/// a NATS micro endpoint name.
///
/// Constrained to the protobuf identifier grammar, which is a subset of the
/// name charset NATS Services (ADR-32) accepts, so one construction satisfies
/// the proto contract and the endpoint registration together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodName(NatsToken);

impl MethodName {
    pub fn from_input(input: &MethodNameInput) -> Result<Self, MethodNameError> {
        let value = input.as_str();
        let token = NatsToken::new(value)?;

        let mut characters = value.chars();
        let leading = characters.next().ok_or(MethodNameError::Empty)?;
        if !leading.is_ascii_alphabetic() && leading != '_' {
            return Err(MethodNameError::LeadingCharacter(leading));
        }
        if let Some(ch) = characters.find(|ch| !ch.is_ascii_alphanumeric() && *ch != '_') {
            return Err(MethodNameError::InvalidCharacter(ch));
        }

        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for MethodName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
