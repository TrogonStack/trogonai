use std::fmt;

use serde::{Deserialize, Serialize};

use crate::BoundedNameError;
use crate::constants::CUSTODY_MAX_NAME_LEN;

/// A namespace, resource name, or uid that is safe to keep in a custody record and to join into a path.
///
/// It holds 1 to [`CUSTODY_MAX_NAME_LEN`] bytes of ASCII letters, digits, `-`, `_` and `.`, and serializes as a
/// JSON string. Construction and deserialization enforce the same grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BoundedName(String);

impl BoundedName {
    pub fn new(value: impl Into<String>) -> Result<Self, BoundedNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(BoundedNameError::Empty);
        }
        if value.len() > CUSTODY_MAX_NAME_LEN {
            return Err(BoundedNameError::TooLong {
                max: CUSTODY_MAX_NAME_LEN,
            });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(BoundedNameError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BoundedName {
    type Error = BoundedNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BoundedName> for String {
    fn from(value: BoundedName) -> Self {
        value.0
    }
}

impl fmt::Display for BoundedName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
