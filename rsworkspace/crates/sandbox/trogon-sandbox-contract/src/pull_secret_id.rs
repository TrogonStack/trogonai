use std::fmt;

use serde::{Deserialize, Serialize};

/// The identifier of a registry pull secret. It names the secret; it never carries its value.
///
/// It serializes as a JSON string and accepts any string on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct PullSecretId(String);

impl PullSecretId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for PullSecretId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<PullSecretId> for String {
    fn from(value: PullSecretId) -> Self {
        value.0
    }
}

impl fmt::Display for PullSecretId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
