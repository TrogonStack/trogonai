//! The snapshot a layer evaluated at, carried so a past decision can be
//! reconstructed (a SpiceDB ZedToken, a projection revision).
//!
//! The value is opaque and compared as a whole, so it is not an
//! [`AuditName`](crate::AuditName): tokens are commonly base64 and carry `=`,
//! `+` and `/`. It still refuses the shapes content takes. Whitespace, quotes,
//! braces, `,`, `;`, `?` and control characters are absent, so a header line, a
//! JSON body, a query string or a prompt does not fit.

use crate::constants::{CONSISTENCY_TOKEN_PUNCTUATION, MAX_CONSISTENCY_TOKEN_BYTES};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConsistencyToken(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConsistencyTokenError {
    #[error("consistency token is empty")]
    Empty,
    #[error("consistency token is {0} bytes, over the {MAX_CONSISTENCY_TOKEN_BYTES}-byte limit")]
    TooLong(usize),
    /// Names the character and never the value.
    #[error("consistency token contains a character a token does not use: {0:?}")]
    InvalidCharacter(char),
}

impl ConsistencyToken {
    pub fn new(value: impl AsRef<str>) -> Result<Self, ConsistencyTokenError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(ConsistencyTokenError::Empty);
        }
        if value.len() > MAX_CONSISTENCY_TOKEN_BYTES {
            return Err(ConsistencyTokenError::TooLong(value.len()));
        }
        if let Some(character) = value
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || CONSISTENCY_TOKEN_PUNCTUATION.contains(*c)))
        {
            return Err(ConsistencyTokenError::InvalidCharacter(character));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
