use std::fmt;

use serde::{Deserialize, Serialize};

/// An encoded, signed JSON Web Token the egress proxy mints for a forwarded request.
///
/// It is as good as the [`OidcClaims`](crate::OidcClaims) it carries to whoever holds it. `Debug` never shows the
/// value and there is no `Display`, so logging a token cannot leak it. Serialization still carries it as a JSON
/// string.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct OidcToken(String);

impl OidcToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Keep it out of logs.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl From<String> for OidcToken {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<OidcToken> for String {
    fn from(value: OidcToken) -> Self {
        value.0
    }
}

impl fmt::Debug for OidcToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OidcToken([REDACTED])")
    }
}
