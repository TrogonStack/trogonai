//! ARD catalog entry metadata value object.

use serde_json::Value;

/// Error returned when [`Metadata`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetadataError {
    #[error("metadata must be a JSON object")]
    NotObject,
    #[error("metadata field '{key}' must be a string, number, boolean, or null")]
    InvalidValue { key: String },
}

/// Validated ARD extension metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Metadata(Value);

impl Metadata {
    pub fn new(value: Value) -> Result<Self, MetadataError> {
        let object = value.as_object().ok_or(MetadataError::NotObject)?;
        for (key, value) in object {
            if !(value.is_string() || value.is_number() || value.is_boolean() || value.is_null()) {
                return Err(MetadataError::InvalidValue { key: key.clone() });
            }
        }
        Ok(Self(Value::Object(object.clone())))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

#[cfg(test)]
mod tests;
