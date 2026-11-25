use std::fmt;

use serde::{Deserialize, Serialize};

/// End-to-end trace correlation identifier the broker assigns when it creates a lease.
///
/// It serializes as a JSON string and accepts any string on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct CorrelationId(String);

impl CorrelationId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for CorrelationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<CorrelationId> for String {
    fn from(value: CorrelationId) -> Self {
        value.0
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
