use std::fmt;
use std::str::FromStr;

use crate::ApiKeyBucketError;

/// Bucket the API key store uses when none is selected.
pub const DEFAULT_BUCKET: &str = "sandbox-api-keys";

/// The name of the key-value bucket that holds API key records.
///
/// It holds ASCII letters, digits, hyphens, and underscores, so it is always a valid bucket name.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ApiKeyBucket(String);

impl ApiKeyBucket {
    pub fn new(value: impl Into<String>) -> Result<Self, ApiKeyBucketError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ApiKeyBucketError::Empty);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ApiKeyBucketError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ApiKeyBucket {
    fn default() -> Self {
        Self(DEFAULT_BUCKET.to_string())
    }
}

impl FromStr for ApiKeyBucket {
    type Err = ApiKeyBucketError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for ApiKeyBucket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
