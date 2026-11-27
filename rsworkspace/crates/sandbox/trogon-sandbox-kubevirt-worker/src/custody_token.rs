use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::CustodyTokenError;

/// An unguessable identifier that tells one allocation, create attempt, or warm claim from another.
///
/// It is a UUID and serializes as its hyphenated string. Construction, parsing, and deserialization enforce it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CustodyToken(Uuid);

impl CustodyToken {
    /// Mints a token from the clock and random bits of a version 7 UUID.
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn new(value: impl AsRef<str>) -> Result<Self, CustodyTokenError> {
        Uuid::parse_str(value.as_ref()).map(Self).map_err(|_| CustodyTokenError)
    }
}

impl TryFrom<String> for CustodyToken {
    type Error = CustodyTokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for CustodyToken {
    type Err = CustodyTokenError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<CustodyToken> for String {
    fn from(value: CustodyToken) -> Self {
        value.to_string()
    }
}

impl fmt::Display for CustodyToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.hyphenated())
    }
}
