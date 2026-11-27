use serde::{Deserialize, Serialize};

/// Whether a write empties the file before writing, so the first chunk of a chunked upload replaces old content.
///
/// It serializes as the JSON boolean `truncate`, `true` for `Truncate`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileWriteTruncation {
    #[default]
    Keep,
    Truncate,
}

impl FileWriteTruncation {
    pub const fn truncates(self) -> bool {
        matches!(self, Self::Truncate)
    }
}

impl From<bool> for FileWriteTruncation {
    fn from(value: bool) -> Self {
        if value { Self::Truncate } else { Self::Keep }
    }
}

impl From<FileWriteTruncation> for bool {
    fn from(value: FileWriteTruncation) -> Self {
        value.truncates()
    }
}
