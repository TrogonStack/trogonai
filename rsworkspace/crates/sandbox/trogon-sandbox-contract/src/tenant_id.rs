use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::TenantIdError;

/// The tenant that owns a lease.
///
/// It holds 1 to [`TenantId::MAX_LEN`] bytes with no control characters and serializes as a JSON
/// string. Construction, parsing, and deserialization all enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TenantId(String);

impl TenantId {
    /// Longest tenant identifier the wire accepts, in bytes.
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, TenantIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(TenantIdError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(TenantIdError::TooLong { max: Self::MAX_LEN });
        }
        if value.chars().any(char::is_control) {
            return Err(TenantIdError::ControlCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TenantId {
    type Error = TenantIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for TenantId {
    type Err = TenantIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<TenantId> for String {
    fn from(value: TenantId) -> Self {
        value.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
