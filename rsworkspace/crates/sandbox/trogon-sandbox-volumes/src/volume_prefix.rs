use std::fmt;
use std::str::FromStr;

use object_store::path::Path as ObjectPath;

use crate::{ManifestKey, ManifestKeyError, VolumePrefixError};

/// A normalized location inside a bucket under which a volume's objects live.
///
/// Surrounding whitespace and slashes are removed. The empty prefix addresses the bucket root.
/// Every segment is valid in an object-store path, so `..` and empty segments are rejected.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct VolumePrefix(ObjectPath);

impl VolumePrefix {
    pub fn new(value: impl AsRef<str>) -> Result<Self, VolumePrefixError> {
        let trimmed = value.as_ref().trim().trim_matches('/');
        Ok(Self(ObjectPath::parse(trimmed)?))
    }

    /// The bucket root.
    pub fn root() -> Self {
        Self(ObjectPath::default())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub fn is_root(&self) -> bool {
        self.as_str().is_empty()
    }

    pub(crate) fn object_path(&self) -> &ObjectPath {
        &self.0
    }

    pub(crate) fn object_path_for(&self, key: &ManifestKey) -> ObjectPath {
        if self.is_root() {
            ObjectPath::from(key.as_str())
        } else {
            ObjectPath::from(format!("{}/{}", self.as_str(), key))
        }
    }

    /// The key of `object` below this prefix. An object outside the prefix, or the prefix itself,
    /// has no key.
    pub(crate) fn relative_key(&self, object: &ObjectPath) -> Result<ManifestKey, ManifestKeyError> {
        let parts: Vec<String> = object
            .prefix_match(&self.0)
            .into_iter()
            .flatten()
            .map(|part| part.as_ref().to_owned())
            .collect();
        ManifestKey::new(parts.join("/"))
    }
}

impl FromStr for VolumePrefix {
    type Err = VolumePrefixError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for VolumePrefix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
