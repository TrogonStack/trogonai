use std::fmt;

use crate::constants::DEFAULT_CLAIM_BUCKET;

/// Why a name cannot be a claim bucket.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClaimBucketError {
    #[error("a claim bucket name must not be empty")]
    Empty,
    #[error("a claim bucket name may not contain {0:?}")]
    InvalidCharacter(char),
}

/// The object-store bucket claim-check payloads live in.
///
/// Constrained to what NATS accepts as a bucket name, so a name the server
/// would refuse fails where it is configured rather than on the first oversized
/// message. It exists mostly to be inseparable from the handle opened on it:
/// see [`ClaimBucketBinding`](super::object_store::ClaimBucketBinding).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClaimBucket(String);

impl ClaimBucket {
    pub fn new(name: impl Into<String>) -> Result<Self, ClaimBucketError> {
        let name = name.into();
        if name.is_empty() {
            return Err(ClaimBucketError::Empty);
        }
        if let Some(invalid) = name
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        {
            return Err(ClaimBucketError::InvalidCharacter(invalid));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ClaimBucket {
    /// The one bucket a trogon deployment uses. Built without going through
    /// [`ClaimBucket::new`] because a constant cannot fail; `the_default_bucket_is_a_valid_name`
    /// is what holds that claim to account.
    fn default() -> Self {
        Self(DEFAULT_CLAIM_BUCKET.to_string())
    }
}

impl fmt::Display for ClaimBucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for ClaimBucket {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

#[cfg(test)]
mod tests;
