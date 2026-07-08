use std::fmt;
use std::sync::Arc;

#[derive(Clone, Eq, PartialEq)]
pub struct CredentialFailureReason(Arc<str>);

impl CredentialFailureReason {
    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialFailureReasonError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(CredentialFailureReasonError::Empty);
        }
        if value.chars().count() > 512 {
            return Err(CredentialFailureReasonError::TooLong);
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialFailureReason").field(&self.as_str()).finish()
    }
}

impl fmt::Display for CredentialFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialFailureReasonError {
    #[error("credential failure reason must not be empty")]
    Empty,
    #[error("credential failure reason exceeds maximum length")]
    TooLong,
}
