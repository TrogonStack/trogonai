use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::StrippedHeaderError;

/// A response header name in its folded form.
///
/// It is lower case, with `_` and `.` read as `-`, so `Set_Cookie`, `set.cookie` and `Set-Cookie` are one header.
/// The input is a non-empty run of ASCII letters, digits, `-`, `_` and `.`. It serializes as a JSON string, and
/// construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StrippedHeader(String);

impl StrippedHeader {
    pub fn new(value: impl Into<String>) -> Result<Self, StrippedHeaderError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !valid {
            return Err(StrippedHeaderError::Invalid);
        }
        let folded = value
            .chars()
            .map(|character| match character {
                '_' | '.' => '-',
                other => other.to_ascii_lowercase(),
            })
            .collect();
        Ok(Self(folded))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StrippedHeader {
    type Error = StrippedHeaderError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for StrippedHeader {
    type Err = StrippedHeaderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<StrippedHeader> for String {
    fn from(header: StrippedHeader) -> Self {
        header.0
    }
}

impl fmt::Display for StrippedHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
