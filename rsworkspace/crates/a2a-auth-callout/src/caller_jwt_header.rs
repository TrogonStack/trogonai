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
        let token = token.into();
        if token.trim().is_empty() {
            return Err(JwtError::Decode("caller JWT header value is empty".into()));
        }
        if token.split('.').count() != 3 {
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
