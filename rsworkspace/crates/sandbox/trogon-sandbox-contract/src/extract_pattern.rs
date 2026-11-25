use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ExtractPatternError;
use crate::js_regex::capture_groups;

/// A regular expression whose capture group 1 is the credential value to mask.
///
/// The credential sits inside a larger value or file, and the rest stays byte for byte intact. The pattern is read in
/// the ECMAScript dialect, so every construct that dialect accepts is accepted here. It has at least one capture
/// group. It serializes as a JSON string, and construction, parsing, and deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExtractPattern(String);

impl ExtractPattern {
    pub fn new(value: impl Into<String>) -> Result<Self, ExtractPatternError> {
        let value = value.into();
        match capture_groups(&value) {
            Err(reason) => Err(ExtractPatternError::Invalid { pattern: value, reason }),
            Ok(0) => Err(ExtractPatternError::WithoutCapture),
            Ok(_) => Ok(Self(value)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExtractPattern {
    type Error = ExtractPatternError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ExtractPattern {
    type Err = ExtractPatternError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<ExtractPattern> for String {
    fn from(pattern: ExtractPattern) -> Self {
        pattern.0
    }
}

impl fmt::Display for ExtractPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
