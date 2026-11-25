use serde::{Deserialize, Serialize};

use crate::{RedactValue, RedactValuesError};

/// The literals a worker redacts from one command's output.
///
/// There are at most [`RedactValues::MAX_VALUES`], and each is at most [`RedactValues::MAX_VALUE_LEN`] bytes.
/// Values too short to be worth redacting are not an error; a worker ignores them. Construction and deserialization
/// both enforce the limits. It serializes as a JSON array of strings, and the values are secrets, so the request that
/// carries them must never be logged or persisted.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<RedactValue>", into = "Vec<RedactValue>")]
pub struct RedactValues(Vec<RedactValue>);

impl RedactValues {
    /// Most values one request may register.
    pub const MAX_VALUES: usize = 64;
    /// Longest single registered value, in bytes.
    pub const MAX_VALUE_LEN: usize = 8 * 1024;

    pub fn new(values: Vec<RedactValue>) -> Result<Self, RedactValuesError> {
        if values.len() > Self::MAX_VALUES {
            return Err(RedactValuesError::TooMany { max: Self::MAX_VALUES });
        }
        if values
            .iter()
            .any(|value| value.expose_secret().len() > Self::MAX_VALUE_LEN)
        {
            return Err(RedactValuesError::TooLong {
                max: Self::MAX_VALUE_LEN,
            });
        }
        Ok(Self(values))
    }

    pub fn as_slice(&self) -> &[RedactValue] {
        &self.0
    }

    pub fn iter(&self) -> std::slice::Iter<'_, RedactValue> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<Vec<RedactValue>> for RedactValues {
    type Error = RedactValuesError;

    fn try_from(values: Vec<RedactValue>) -> Result<Self, Self::Error> {
        Self::new(values)
    }
}

impl From<RedactValues> for Vec<RedactValue> {
    fn from(values: RedactValues) -> Self {
        values.0
    }
}
