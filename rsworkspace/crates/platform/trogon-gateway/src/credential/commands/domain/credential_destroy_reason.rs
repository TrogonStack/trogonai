use std::fmt;
use std::sync::Arc;

use super::constants::MAX_DESTROY_REASON_LEN;

#[derive(Clone, Eq, PartialEq)]
pub struct CredentialDestroyReason(Arc<str>);

impl CredentialDestroyReason {
    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialDestroyReasonError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(CredentialDestroyReasonError::Empty);
        }
        if value.chars().count() > MAX_DESTROY_REASON_LEN {
            return Err(CredentialDestroyReasonError::TooLong);
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialDestroyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialDestroyReason").field(&self.as_str()).finish()
    }
}

impl fmt::Display for CredentialDestroyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialDestroyReasonError {
    #[error("credential destroy reason must not be empty")]
    Empty,
    #[error("credential destroy reason exceeds maximum length")]
    TooLong,
}
