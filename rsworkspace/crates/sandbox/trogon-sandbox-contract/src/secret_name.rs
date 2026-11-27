use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::SecretNameError;

/// Identifies a secret binding and its sentinel.
///
/// Keying on the name rather than the value means two bindings that hold the same secret get distinct sentinels. It
/// holds one or more ASCII letters, digits, `.`, `_`, or `-`, and serializes as a JSON string. Construction,
/// parsing, and deserialization all enforce the grammar. A name never carries a secret value.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecretName(String);

impl SecretName {
    pub fn new(value: impl Into<String>) -> Result<Self, SecretNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecretNameError::Empty);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(SecretNameError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SecretName {
    type Error = SecretNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SecretName {
    type Err = SecretNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<SecretName> for String {
    fn from(name: SecretName) -> Self {
        name.0
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
