use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::EnvVarNameError;

/// The name of an environment variable inside the sandbox.
///
/// It starts with an ASCII letter or an underscore and continues with ASCII letters, digits, or underscores, and it
/// has no length limit. It serializes as a JSON string, and construction, parsing, and deserialization all enforce
/// the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EnvVarName(String);

impl EnvVarName {
    pub fn new(value: impl Into<String>) -> Result<Self, EnvVarNameError> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if valid {
            Ok(Self(value))
        } else {
            Err(EnvVarNameError::Invalid(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EnvVarName {
    type Error = EnvVarNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for EnvVarName {
    type Err = EnvVarNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<EnvVarName> for String {
    fn from(name: EnvVarName) -> Self {
        name.0
    }
}

impl fmt::Display for EnvVarName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
