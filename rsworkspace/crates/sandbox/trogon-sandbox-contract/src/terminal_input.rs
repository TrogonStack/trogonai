use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Base64Data, TerminalInputError};

/// The bytes of one write to a terminal's input, as base64.
///
/// The decoded bytes must not exceed [`TerminalInput::MAX_BYTES`]. An encoded text longer than any valid
/// encoding of that many bytes is refused before it is decoded. Construction, parsing, and deserialization all
/// enforce both rules.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TerminalInput(Base64Data);

impl TerminalInput {
    /// Largest input write, in bytes before base64 encoding.
    pub const MAX_BYTES: usize = 64 * 1024;

    pub fn new(value: impl Into<String>) -> Result<Self, TerminalInputError> {
        let value = value.into();
        if value.len() > Self::MAX_BYTES.div_ceil(3) * 4 {
            return Err(TerminalInputError::TooLarge { max: Self::MAX_BYTES });
        }
        let data = Base64Data::new(value).map_err(|source| TerminalInputError::InvalidBase64 { source })?;
        let text = data.as_str();
        let padding = text.bytes().rev().take_while(|byte| *byte == b'=').count();
        if text.len() / 4 * 3 - padding > Self::MAX_BYTES {
            return Err(TerminalInputError::TooLarge { max: Self::MAX_BYTES });
        }
        Ok(Self(data))
    }

    pub fn as_base64(&self) -> &Base64Data {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for TerminalInput {
    type Error = TerminalInputError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for TerminalInput {
    type Err = TerminalInputError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<TerminalInput> for String {
    fn from(value: TerminalInput) -> Self {
        value.0.into()
    }
}

impl fmt::Display for TerminalInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.as_str())
    }
}
