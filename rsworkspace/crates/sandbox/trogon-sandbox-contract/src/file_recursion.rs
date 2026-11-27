use serde::{Deserialize, Serialize};

/// Whether a listing, removal, or watch covers a directory's whole tree or only the path itself.
///
/// It serializes as the JSON boolean `recursive`, `true` for `Recursive`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileRecursion {
    #[default]
    Flat,
    Recursive,
}

impl FileRecursion {
    pub const fn is_recursive(self) -> bool {
        matches!(self, Self::Recursive)
    }
}

impl From<bool> for FileRecursion {
    fn from(value: bool) -> Self {
        if value { Self::Recursive } else { Self::Flat }
    }
}

impl From<FileRecursion> for bool {
    fn from(value: FileRecursion) -> Self {
        value.is_recursive()
    }
}
