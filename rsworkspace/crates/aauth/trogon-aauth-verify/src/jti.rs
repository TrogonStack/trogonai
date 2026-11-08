use crate::constants::MAX_JTI_BYTES;

/// The client-generated identifier that makes a signature single-use
/// (ADR#0051 Decision 2), carried in the `nonce` signature parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jti(String);

impl Jti {
    /// # Errors
    ///
    /// [`InvalidJtiError`] when `value` is empty or longer than
    /// [`MAX_JTI_BYTES`].
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidJtiError> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidJtiError::Empty);
        }
        if value.len() > MAX_JTI_BYTES {
            return Err(InvalidJtiError::TooLong { len: value.len() });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidJtiError {
    #[error("jti is empty")]
    Empty,
    #[error("jti is {len} bytes, longer than the {max} a verifier records", max = MAX_JTI_BYTES)]
    TooLong { len: usize },
}
