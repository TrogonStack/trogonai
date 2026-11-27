use serde::{Deserialize, Serialize};

/// Whether the caller wrote a trailing separator on a literal path entry.
///
/// It matters only to a Windows deny on a path that is not there yet: the placeholder is then a directory, not a
/// file. It serializes as a JSON boolean, `true` for `Directory`, and an omitted value reads as `Unmarked`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum DirectoryMarker {
    #[default]
    Unmarked,
    Directory,
}

impl DirectoryMarker {
    pub const fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Reports whether the marker is the default, so the wire can leave it out.
    pub const fn is_unmarked(&self) -> bool {
        matches!(self, Self::Unmarked)
    }
}

impl From<bool> for DirectoryMarker {
    fn from(directory: bool) -> Self {
        if directory { Self::Directory } else { Self::Unmarked }
    }
}

impl From<DirectoryMarker> for bool {
    fn from(marker: DirectoryMarker) -> Self {
        marker.is_directory()
    }
}
