use std::fmt;

use reqwest::header::HeaderValue;
use serde::Deserialize;

use crate::SpaceTokenError;

/// A bearer token for SPACE, kept as a sensitive header value so it never reaches
/// debug output.
#[derive(Clone, Deserialize)]
#[serde(try_from = "String")]
pub struct SpaceToken(HeaderValue);

impl SpaceToken {
    /// Parses a token that is non-empty and valid in a header.
    pub fn parse(token: &str) -> Result<Self, SpaceTokenError> {
        if token.is_empty() {
            return Err(SpaceTokenError::Empty);
        }
        let mut value =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| SpaceTokenError::NotAHeaderValue)?;
        value.set_sensitive(true);
        Ok(Self(value))
    }

    /// The `Authorization` header value.
    pub(crate) fn authorization(&self) -> HeaderValue {
        self.0.clone()
    }
}

impl TryFrom<String> for SpaceToken {
    type Error = SpaceTokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl fmt::Debug for SpaceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SpaceToken(<redacted>)")
    }
}
