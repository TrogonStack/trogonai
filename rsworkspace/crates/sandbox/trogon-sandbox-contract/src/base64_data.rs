use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::Base64DataError;

/// Bytes written as standard base64 with padding.
///
/// Construction, parsing, and deserialization decode the text once to reject anything that is not valid base64, and
/// the text is kept as written, so it serializes unchanged.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Base64Data(String);

impl Base64Data {
    pub fn new(value: impl Into<String>) -> Result<Self, Base64DataError> {
        let value = value.into();
        match STANDARD.decode(&value) {
            Ok(_) => Ok(Self(value)),
            Err(source) => Err(Base64DataError { source }),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Base64Data {
    type Error = Base64DataError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for Base64Data {
    type Err = Base64DataError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<Base64Data> for String {
    fn from(data: Base64Data) -> Self {
        data.0
    }
}

impl fmt::Display for Base64Data {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
