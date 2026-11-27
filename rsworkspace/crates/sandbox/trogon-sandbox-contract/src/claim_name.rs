use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ClaimNameError;

/// A top-level payload claim of a JSON web token to mask.
///
/// It is any non-empty string. It serializes as a JSON string, and construction, parsing, and deserialization all
/// enforce the rule.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClaimName(String);

impl ClaimName {
    pub fn new(value: impl Into<String>) -> Result<Self, ClaimNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ClaimNameError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ClaimName {
    type Error = ClaimNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ClaimName {
    type Err = ClaimNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ClaimName> for String {
    fn from(name: ClaimName) -> Self {
        name.0
    }
}

impl fmt::Display for ClaimName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
