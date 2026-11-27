use std::fmt;
use std::str::FromStr;

use crate::KmsKeyIdError;

/// A KMS key identifier: a non-empty token without whitespace or control characters.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct KmsKeyId(String);

impl KmsKeyId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, KmsKeyIdError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(KmsKeyIdError::Empty);
        }
        if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(KmsKeyIdError::InvalidCharacter);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for KmsKeyId {
    type Err = KmsKeyIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for KmsKeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
