/// A bucket name does not satisfy the rules [`ApiKeyBucket`](crate::ApiKeyBucket) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ApiKeyBucketError {
    #[error("API key bucket must not be empty")]
    Empty,
    #[error("API key bucket must contain only ASCII letters, digits, hyphens or underscores")]
    InvalidCharacter,
}
