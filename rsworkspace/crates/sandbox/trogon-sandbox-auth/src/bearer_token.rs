use std::fmt;

use secrecy::{ExposeSecret, SecretString};

use crate::{ApiKeyHash, BearerTokenError};

/// A bearer API key presented by a caller.
///
/// It holds one or more visible ASCII characters. The value is wrapped so it never appears in
/// `Debug` output or error messages; read it with [`BearerToken::expose_secret`] only where it
/// must leave the process, and look keys up by [`BearerToken::hash`].
pub struct BearerToken(SecretString);

impl BearerToken {
    pub fn new(value: impl Into<String>) -> Result<Self, BearerTokenError> {
        let value = value.into();
        if value.is_empty() {
            return Err(BearerTokenError::Empty);
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(BearerTokenError::InvalidCharacter);
        }
        Ok(Self(SecretString::from(value)))
    }

    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }

    /// The hash the key store indexes this token under.
    pub fn hash(&self) -> ApiKeyHash {
        ApiKeyHash::of(self)
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken(***)")
    }
}
