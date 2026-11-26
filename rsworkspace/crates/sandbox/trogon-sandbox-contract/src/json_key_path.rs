use serde::{Deserialize, Serialize};

use crate::JsonKeyPathError;

/// Object keys leading to a string value in a JSON file, outermost first.
///
/// It holds at least one key. It serializes as a JSON array of strings, and construction and deserialization both
/// reject an empty array.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct JsonKeyPath(Vec<String>);

impl JsonKeyPath {
    pub fn new(keys: Vec<String>) -> Result<Self, JsonKeyPathError> {
        if keys.is_empty() {
            return Err(JsonKeyPathError::Empty);
        }
        Ok(Self(keys))
    }

    pub fn keys(&self) -> &[String] {
        &self.0
    }
}

impl TryFrom<Vec<String>> for JsonKeyPath {
    type Error = JsonKeyPathError;

    fn try_from(keys: Vec<String>) -> Result<Self, Self::Error> {
        Self::new(keys)
    }
}

impl From<JsonKeyPath> for Vec<String> {
    fn from(path: JsonKeyPath) -> Self {
        path.0
    }
}
