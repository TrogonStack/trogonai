use std::fmt;
use std::str::FromStr;

use reqwest::header::HeaderName;

/// HTTP header name for an outbound idempotency key (alternative to the default [`Idempotency-Key`](https://datatracker.ietf.org/doc/html/draft-ietf-httpapi-idempotency-key-header/) header).
#[derive(Clone, Eq, PartialEq)]
pub struct IdempotencyKeyHeader(HeaderName);

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum IdempotencyKeyHeaderError {
    InvalidToken,
}

impl IdempotencyKeyHeader {
    pub fn new(name: HeaderName) -> Self {
        Self(name)
    }

    pub fn as_http(&self) -> &HeaderName {
        &self.0
    }
}

impl TryFrom<&str> for IdempotencyKeyHeader {
    type Error = IdempotencyKeyHeaderError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let name =
            HeaderName::from_str(value.trim()).map_err(|_| IdempotencyKeyHeaderError::InvalidToken)?;
        Ok(Self(name))
    }
}

impl fmt::Display for IdempotencyKeyHeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken => write!(f, "invalid HTTP header name token for idempotency key carrier"),
        }
    }
}

impl std::error::Error for IdempotencyKeyHeaderError {}

impl fmt::Display for IdempotencyKeyHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

impl fmt::Debug for IdempotencyKeyHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IdempotencyKeyHeader").field(&self.0.as_str()).finish()
    }
}
