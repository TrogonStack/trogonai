//! The JSON Pointer a validation choice names its schema node by.

use std::fmt;

/// An RFC 6901 JSON Pointer into a tool's input schema (`""` is the schema itself, `"/properties/x"` a
/// property).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaPointer(String);

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "`{0}` is not a JSON Pointer: it is neither empty nor starts with `/`, or has a `~` not followed by `0` or `1`"
)]
pub struct SchemaPointerError(String);

impl SchemaPointer {
    pub fn parse(pointer: impl Into<String>) -> Result<Self, SchemaPointerError> {
        let pointer = pointer.into();
        let escapes_valid = pointer
            .split('~')
            .skip(1)
            .all(|rest| rest.starts_with('0') || rest.starts_with('1'));
        if (pointer.is_empty() || pointer.starts_with('/')) && escapes_valid {
            Ok(Self(pointer))
        } else {
            Err(SchemaPointerError(pointer))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchemaPointer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
