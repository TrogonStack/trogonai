//! The annotated protobuf `service`'s name, which is both a subject token and
//! the registered NATS micro service's name (ADR 0016 §1, §2).

use trogon_nats::{NatsToken, SubjectTokenViolationError};

use crate::service_name_input::ServiceNameInput;

/// Why a [`ServiceName`] could not be constructed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ServiceNameError {
    #[error("service name must not be empty")]
    Empty,
    #[error("service name must start with an ASCII letter or underscore, found {0:?}")]
    LeadingCharacter(char),
    #[error("service name contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("service name is too long: {0} characters")]
    TooLong(usize),
}

impl From<SubjectTokenViolationError> for ServiceNameError {
    fn from(violation: SubjectTokenViolationError) -> Self {
        match violation {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// A protobuf service name that is safe to use as both a subject token and a
/// NATS micro service name.
///
/// Constrained to the protobuf identifier grammar, which is a subset of the
/// name charset NATS Services (ADR-32) accepts, so one construction satisfies
/// the proto contract and the micro registration together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceName(NatsToken);

impl ServiceName {
    pub fn from_input(input: &ServiceNameInput) -> Result<Self, ServiceNameError> {
        let value = input.as_str();
        let token = NatsToken::new(value)?;

        let mut characters = value.chars();
        let leading = characters.next().ok_or(ServiceNameError::Empty)?;
        if !leading.is_ascii_alphabetic() && leading != '_' {
            return Err(ServiceNameError::LeadingCharacter(leading));
        }
        if let Some(ch) = characters.find(|ch| !ch.is_ascii_alphanumeric() && *ch != '_') {
            return Err(ServiceNameError::InvalidCharacter(ch));
        }

        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for ServiceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
