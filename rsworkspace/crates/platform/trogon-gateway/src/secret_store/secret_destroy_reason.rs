use std::fmt;
use std::sync::Arc;

#[derive(Clone, Eq, PartialEq)]
pub struct SecretDestroyReason(Arc<str>);

impl SecretDestroyReason {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SecretDestroyReasonError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(SecretDestroyReasonError::Empty);
        }
        if value.chars().count() > 512 {
            return Err(SecretDestroyReasonError::TooLong);
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretDestroyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SecretDestroyReason").field(&self.as_str()).finish()
    }
}

impl fmt::Display for SecretDestroyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SecretDestroyReasonError {
    #[error("secret destroy reason must not be empty")]
    Empty,
    #[error("secret destroy reason exceeds maximum length")]
    TooLong,
}
