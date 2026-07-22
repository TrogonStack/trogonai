use std::fmt;

use trogon_std::{EmptySecretError, SecretString};

#[derive(Clone)]
pub struct SentryClientSecret(SecretString);

impl SentryClientSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SentryClientSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SentryClientSecret(****)")
    }
}

#[cfg(test)]
mod tests;
