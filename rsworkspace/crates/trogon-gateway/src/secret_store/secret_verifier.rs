use std::fmt;

use trogon_std::{EmptySecret, SecretString};

#[derive(Clone)]
pub struct SecretVerifier(SecretString);

impl SecretVerifier {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SecretVerifierError> {
        SecretString::new(value).map(Self).map_err(SecretVerifierError::Empty)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretVerifier(***)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretVerifierError {
    #[error("{0}")]
    Empty(#[source] EmptySecret),
}
