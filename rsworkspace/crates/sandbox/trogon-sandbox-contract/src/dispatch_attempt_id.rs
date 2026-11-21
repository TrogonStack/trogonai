use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::TokenError;
use crate::token::validate_token;

/// A dispatch-attempt identifier containing 1 to 128 ASCII letters, digits, `_`, or `-`.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DispatchAttemptId(String);

impl DispatchAttemptId {
    pub fn new(value: impl Into<String>) -> Result<Self, TokenError> {
        let value = value.into();
        validate_token(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DispatchAttemptId {
    type Error = TokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for DispatchAttemptId {
    type Err = TokenError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<DispatchAttemptId> for String {
    fn from(value: DispatchAttemptId) -> Self {
        value.0
    }
}

impl fmt::Display for DispatchAttemptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
