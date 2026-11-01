use std::fmt;

use trogon_std::{EmptySecretError, SecretString};

#[derive(Clone)]
pub struct PlaidSecret(SecretString);

impl PlaidSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<SecretString> for PlaidSecret {
    fn from(secret: SecretString) -> Self {
        Self(secret)
    }
}

impl fmt::Debug for PlaidSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaidSecret(****)")
    }
}

#[cfg(test)]
mod tests;
