use std::fmt;

use subtle::{Choice, ConstantTimeEq};

mod constants;
pub use constants::MAX_TOKEN_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidProxyTokenError {
    #[error("proxy token must not be empty")]
    Empty,
    #[error("proxy token must be at most {MAX_TOKEN_BYTES} bytes")]
    TooLong,
    #[error("proxy token must not contain control characters or whitespace")]
    ForbiddenCharacter,
}

/// The per-lease secret a client presents to use the explicit proxy. Bounded
/// to what RFC 1929 can carry, never printed, and compared in constant time
/// over a fixed-size buffer so neither content nor length is observable
/// through timing.
#[derive(Clone)]
pub struct ProxyToken(Box<[u8]>);

impl ProxyToken {
    pub fn new(token: &str) -> Result<Self, InvalidProxyTokenError> {
        if token.is_empty() {
            return Err(InvalidProxyTokenError::Empty);
        }
        if token.len() > MAX_TOKEN_BYTES {
            return Err(InvalidProxyTokenError::TooLong);
        }
        if token.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(InvalidProxyTokenError::ForbiddenCharacter);
        }
        Ok(Self(token.as_bytes().into()))
    }

    pub fn verify(&self, presented: &[u8]) -> bool {
        let fits = Choice::from((presented.len() <= MAX_TOKEN_BYTES) as u8);
        let (expected, expected_len) = Self::padded(&self.0);
        let (offered, offered_len) = Self::padded(&presented[..presented.len().min(MAX_TOKEN_BYTES)]);
        let same = expected.ct_eq(&offered) & expected_len.ct_eq(&offered_len);
        bool::from(same & fits)
    }

    fn padded(bytes: &[u8]) -> ([u8; MAX_TOKEN_BYTES], u16) {
        let mut buffer = [0u8; MAX_TOKEN_BYTES];
        buffer[..bytes.len()].copy_from_slice(bytes);
        (buffer, bytes.len() as u16)
    }
}

impl PartialEq for ProxyToken {
    fn eq(&self, other: &Self) -> bool {
        self.verify(&other.0)
    }
}

impl Eq for ProxyToken {}

impl fmt::Debug for ProxyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProxyToken(<redacted>)")
    }
}

#[cfg(test)]
mod tests;
