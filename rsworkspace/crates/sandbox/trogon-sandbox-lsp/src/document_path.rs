use std::fmt;

use crate::document_path_error::DocumentPathError;

/// Path of a document relative to the [`ProjectDir`](crate::ProjectDir).
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct DocumentPath(String);

impl DocumentPath {
    pub fn new(path: impl Into<String>) -> Result<Self, DocumentPathError> {
        let path = path.into();
        if path.is_empty() {
            return Err(DocumentPathError::Empty);
        }
        if path.starts_with('/') {
            return Err(DocumentPathError::Absolute { path });
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocumentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for DocumentPath {
    type Error = DocumentPathError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for DocumentPath {
    type Error = DocumentPathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
