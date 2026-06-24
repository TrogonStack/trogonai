use std::fmt;

use trogon_std::{EmptySecret, SecretString};

#[derive(Clone)]
pub struct NotionVerificationToken(SecretString);

impl NotionVerificationToken {
    pub fn new(value: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for NotionVerificationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NotionVerificationToken(****)")
    }
}

#[cfg(test)]
mod tests;
