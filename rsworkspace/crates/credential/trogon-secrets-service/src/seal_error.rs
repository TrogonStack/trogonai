use super::SealedValueError;

/// A value could not be sealed to the requester.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("value could not be sealed: {message}")]
pub struct SealError {
    message: String,
}

impl SealError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<nkeys::error::Error> for SealError {
    fn from(error: nkeys::error::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<SealedValueError> for SealError {
    fn from(error: SealedValueError) -> Self {
        Self::new(error.to_string())
    }
}

#[cfg(test)]
mod tests;
