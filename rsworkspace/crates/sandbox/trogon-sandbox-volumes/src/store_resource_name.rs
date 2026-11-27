use std::fmt;
use std::str::FromStr;

use crate::StoreResourceNameError;

/// The name of a storage account, container, or bucket: a non-empty token without whitespace or control characters.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct StoreResourceName(String);

impl StoreResourceName {
    pub fn new(value: impl AsRef<str>) -> Result<Self, StoreResourceNameError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(StoreResourceNameError::Empty);
        }
        if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(StoreResourceNameError::InvalidCharacter);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for StoreResourceName {
    type Err = StoreResourceNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for StoreResourceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
