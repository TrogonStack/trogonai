use std::fmt;
use std::str::FromStr;

use http::HeaderName;

/// HTTP header name for an outbound idempotency key (alternative to the default [`Idempotency-Key`](https://datatracker.ietf.org/doc/html/draft-ietf-httpapi-idempotency-key-header/) header).
#[derive(Clone, Eq, PartialEq)]
pub struct IdempotencyKeyHeader(HeaderName);

#[derive(Clone, Eq, PartialEq, Debug, thiserror::Error)]
pub enum IdempotencyKeyHeaderError {
    #[error("invalid HTTP header name token for idempotency key carrier")]
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
        let name = HeaderName::from_str(value.trim()).map_err(|_| IdempotencyKeyHeaderError::InvalidToken)?;
        Ok(Self(name))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_accepts_valid_header_name() {
        let header = IdempotencyKeyHeader::try_from("Idempotency-Key").unwrap();
        assert_eq!(header.as_http().as_str(), "idempotency-key");
        assert_eq!(header.to_string(), "idempotency-key");
    }

    #[test]
    fn try_from_trims_whitespace_before_parsing() {
        let header = IdempotencyKeyHeader::try_from("  X-Push-Key  ").unwrap();
        assert_eq!(header.as_http().as_str(), "x-push-key");
    }

    #[test]
    fn try_from_rejects_invalid_token() {
        let err = IdempotencyKeyHeader::try_from("not a valid header").unwrap_err();
        assert!(matches!(err, IdempotencyKeyHeaderError::InvalidToken));
    }

    #[test]
    fn error_display_describes_invalid_token() {
        let err = IdempotencyKeyHeaderError::InvalidToken;
        assert!(err.to_string().contains("invalid HTTP header name"));
        assert!(format!("{err:?}").contains("InvalidToken"));
    }

    #[test]
    fn new_wraps_existing_header_name() {
        let name = HeaderName::from_static("x-trace");
        let header = IdempotencyKeyHeader::new(name);
        assert_eq!(header.as_http().as_str(), "x-trace");
    }

    #[test]
    fn debug_shows_wrapped_header_value() {
        let header = IdempotencyKeyHeader::try_from("Idempotency-Key").unwrap();
        assert!(format!("{header:?}").contains("idempotency-key"));
    }
}
