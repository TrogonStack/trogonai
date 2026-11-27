use std::fmt;

use crate::document_path::DocumentPath;
use crate::path_encoding::{file_uri, normalize};
use crate::project_dir_error::ProjectDirError;

/// Absolute POSIX directory inside the sandbox that the language server treats as its root.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ProjectDir(String);

impl ProjectDir {
    pub fn new(path: impl AsRef<str>) -> Result<Self, ProjectDirError> {
        let path = path.as_ref();
        if !path.starts_with('/') {
            return Err(ProjectDirError::NotAbsolute { path: path.to_owned() });
        }
        Ok(Self(normalize(path)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Last path segment, or `/` for the filesystem root.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').find(|segment| !segment.is_empty()).unwrap_or("/")
    }

    pub fn resolve(&self, path: &DocumentPath) -> String {
        normalize(&format!("{}/{}", self.0, path.as_str()))
    }

    pub fn root_uri(&self) -> String {
        file_uri(&self.0)
    }

    pub fn document_uri(&self, path: &DocumentPath) -> String {
        file_uri(&self.resolve(path))
    }
}

impl fmt::Display for ProjectDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
