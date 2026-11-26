use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::LeaseRequestKeyError;

/// The caller-chosen key that makes a lease request idempotent.
///
/// It holds 1 to [`LeaseRequestKey::MAX_LEN`] bytes of ASCII letters, digits, `_`, or `-` and
/// serializes as a JSON string. Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LeaseRequestKey(String);

impl LeaseRequestKey {
    /// Longest request key the wire accepts, in bytes.
    pub const MAX_LEN: usize = 256;

    pub fn new(value: impl Into<String>) -> Result<Self, LeaseRequestKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(LeaseRequestKeyError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(LeaseRequestKeyError::TooLong { max: Self::MAX_LEN });
        }
        if let Some(index) = value
            .bytes()
            .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
        {
            return Err(LeaseRequestKeyError::InvalidCharacter { index });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LeaseRequestKey {
    type Error = LeaseRequestKeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for LeaseRequestKey {
    type Err = LeaseRequestKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<LeaseRequestKey> for String {
    fn from(value: LeaseRequestKey) -> Self {
        value.0
    }
}

impl fmt::Display for LeaseRequestKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
