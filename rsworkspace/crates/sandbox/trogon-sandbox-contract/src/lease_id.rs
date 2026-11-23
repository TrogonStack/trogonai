use std::fmt;

use serde::{Deserialize, Serialize};

/// The identifier the broker assigns to a sandbox lease.
///
/// It serializes as a JSON string and accepts any string on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct LeaseId(String);

impl LeaseId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for LeaseId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<LeaseId> for String {
    fn from(value: LeaseId) -> Self {
        value.0
    }
}

impl fmt::Display for LeaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
