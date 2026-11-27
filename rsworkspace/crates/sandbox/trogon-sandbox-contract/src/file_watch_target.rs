use serde::{Deserialize, Serialize};

/// Whether a watch event concerns a directory or a file.
///
/// It serializes as the JSON boolean `is_dir`, `true` for `Directory`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileWatchTarget {
    #[default]
    File,
    Directory,
}

impl FileWatchTarget {
    pub const fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }
}

impl From<bool> for FileWatchTarget {
    fn from(value: bool) -> Self {
        if value { Self::Directory } else { Self::File }
    }
}

impl From<FileWatchTarget> for bool {
    fn from(value: FileWatchTarget) -> Self {
        value.is_directory()
    }
}
