use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use trogon_sandbox_contract::ShortLivedToken;

/// A credential value held in memory only.
///
/// It is never serialized, never displayed, and redacted in `Debug`.
#[derive(Clone)]
pub struct RealValue(SecretString);

impl RealValue {
    pub fn new(value: String) -> Self {
        Self(SecretString::from(value))
    }

    /// The secret value. Keep it out of logs.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl From<ShortLivedToken> for RealValue {
    fn from(token: ShortLivedToken) -> Self {
        Self::new(token.expose_secret().to_string())
    }
}

impl fmt::Debug for RealValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RealValue(***)")
    }
}
