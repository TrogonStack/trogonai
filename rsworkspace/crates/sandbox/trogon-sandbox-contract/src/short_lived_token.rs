use std::fmt;

use serde::{Deserialize, Serialize};

/// A short-lived token the credential broker issues.
///
/// `Debug` never shows the value and there is no `Display`, so logging a token cannot leak it. Serialization still
/// carries it as a JSON string, because the requester needs it.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ShortLivedToken(String);

impl ShortLivedToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Keep it out of logs.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl From<String> for ShortLivedToken {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<ShortLivedToken> for String {
    fn from(value: ShortLivedToken) -> Self {
        value.0
    }
}

impl fmt::Debug for ShortLivedToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ShortLivedToken([REDACTED])")
    }
}
