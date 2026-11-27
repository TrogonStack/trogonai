use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use crate::ManifestKeyError;

/// A relative, forward-slash separated file key inside a volume.
///
/// Every segment is non-empty and is neither `.` nor `..`, and the key contains no backslash,
/// so joining a key onto a directory never escapes it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ManifestKey(String);

impl ManifestKey {
    pub fn new(value: impl AsRef<str>) -> Result<Self, ManifestKeyError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(ManifestKeyError::Empty);
        }
        if value.contains('\\') {
            return Err(ManifestKeyError::Backslash);
        }
        for segment in value.split('/') {
            match segment {
                "" => return Err(ManifestKeyError::EmptySegment),
                "." | ".." => return Err(ManifestKeyError::RelativeSegment(segment.to_owned())),
                _ => {}
            }
        }
        Ok(Self(value.to_owned()))
    }

    /// Builds a key from a path relative to a directory, such as the result of stripping a root.
    pub fn from_relative_path(path: &Path) -> Result<Self, ManifestKeyError> {
        let mut segments = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    segments.push(name.to_str().ok_or(ManifestKeyError::NonUnicode)?);
                }
                _ => return Err(ManifestKeyError::NotRelative),
            }
        }
        Self::new(segments.join("/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key resolved under `base` using the platform path separator.
    pub fn to_path(&self, base: &Path) -> PathBuf {
        self.0
            .split('/')
            .fold(base.to_path_buf(), |path, segment| path.join(segment))
    }
}

impl FromStr for ManifestKey {
    type Err = ManifestKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for ManifestKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
