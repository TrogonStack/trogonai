use std::fmt;

use serde::{Deserialize, Serialize};

/// One token the credential broker issued, used to revoke it or audit it.
///
/// It serializes as a JSON string and accepts any string on deserialization.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct TokenId(String);

impl TokenId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for TokenId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<TokenId> for String {
    fn from(value: TokenId) -> Self {
        value.0
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
