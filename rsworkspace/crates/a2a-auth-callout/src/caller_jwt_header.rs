use std::fmt;

use crate::jwt::{JwtError, MintedUserJwt};

pub const CALLER_JWT_HEADER_NAME: &str = "A2a-Caller-Jwt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerJwtHeaderValue(String);

impl CallerJwtHeaderValue {
    pub fn from_minted(jwt: &MintedUserJwt) -> Self {
        Self(jwt.as_str().to_owned())
    }

    pub fn parse(token: impl Into<String>) -> Result<Self, JwtError> {
        // Trim once on entry — JWT decoders downstream choke on leading/trailing
        // whitespace, and storing the original value let space-padded but
        // otherwise valid headers pass `parse()` only to break later in
        // `as_str()`-consuming code paths.
        let token = token.into().trim().to_owned();
        if token.is_empty() {
            return Err(JwtError::Decode("caller JWT header value is empty".into()));
        }
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
            return Err(JwtError::Decode("caller JWT header value is not a compact JWT".into()));
        }
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CallerJwtHeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
