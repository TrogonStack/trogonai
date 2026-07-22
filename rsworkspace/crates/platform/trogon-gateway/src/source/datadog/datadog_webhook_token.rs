use std::fmt;

use trogon_std::{EmptySecret, SecretString};

#[derive(Clone)]
pub struct DatadogWebhookToken(SecretString);

impl DatadogWebhookToken {
    pub fn new(value: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DatadogWebhookToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DatadogWebhookToken(****)")
    }
}

#[cfg(test)]
mod tests;
