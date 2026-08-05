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

/// The characters a bucket name is drawn from.
fn is_permitted(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

impl ClaimBucket {
    pub fn new(name: impl Into<String>) -> Result<Self, ClaimBucketError> {
        let name = name.into();
        if name.is_empty() {
            return Err(ClaimBucketError::Empty);
        }
        if let Some(invalid) = name.chars().find(|c| !is_permitted(*c)) {
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
    /// [`ClaimBucket::new`] because the factory is fallible and this cannot be:
    /// the constant is held to the factory's rule by the tests, so a name the
    /// factory would refuse fails there rather than in a deployment.
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

/// The bucket a claim message says it was written to, exactly as the header
/// spelled it.
///
/// Whoever published the message wrote this, so it is input rather than a name:
/// it may be a bucket this deployment does not open, or not a legal bucket name
/// at all. Keeping it a type of its own means nothing can pass it where a
/// [`ClaimBucket`] belongs without going through [`ClaimBucketHeader::parse`],
/// while the text an operator has to read still survives into the error.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClaimBucketHeader(String);

impl ClaimBucketHeader {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn parse(&self) -> Result<ClaimBucket, ClaimBucketError> {
        ClaimBucket::new(self.0.as_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClaimBucketHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
