use std::fmt;

use serde::{Deserialize, Serialize};

/// The service account that performed a change, such as moving a snapshot tag.
///
/// It serializes as a JSON string and accepts any string on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ServiceAccountId(String);

impl ServiceAccountId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ServiceAccountId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<ServiceAccountId> for String {
    fn from(value: ServiceAccountId) -> Self {
        value.0
    }
}

impl fmt::Display for ServiceAccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
