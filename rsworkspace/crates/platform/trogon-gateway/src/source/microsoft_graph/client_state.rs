use std::fmt;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use trogon_std::{EmptySecretError, SecretString};

#[derive(Clone)]
pub struct MicrosoftGraphClientState(SecretString);

impl MicrosoftGraphClientState {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn matches(&self, provided: &str) -> bool {
        let expected = Sha256::digest(self.as_str().as_bytes());
        let provided = Sha256::digest(provided.as_bytes());
        expected.as_slice().ct_eq(provided.as_slice()).unwrap_u8() == 1
    }
}

impl fmt::Debug for MicrosoftGraphClientState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MicrosoftGraphClientState(****)")
    }
}

#[cfg(test)]
mod tests;
