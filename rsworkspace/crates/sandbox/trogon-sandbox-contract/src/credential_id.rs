use std::fmt;

use serde::{Deserialize, Serialize};

/// A stored credential the broker issues tokens from.
///
/// It serializes as a JSON string and accepts any string on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct CredentialId(String);

impl CredentialId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for CredentialId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<CredentialId> for String {
    fn from(value: CredentialId) -> Self {
        value.0
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
