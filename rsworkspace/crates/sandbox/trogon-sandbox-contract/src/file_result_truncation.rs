use serde::{Deserialize, Serialize};

/// Whether a reply was cut short by a size or entry ceiling.
///
/// It serializes as the JSON boolean `truncated`, `true` for `Truncated`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileResultTruncation {
    #[default]
    Complete,
    Truncated,
}

impl FileResultTruncation {
    pub const fn is_truncated(self) -> bool {
        matches!(self, Self::Truncated)
    }
}

impl From<bool> for FileResultTruncation {
    fn from(value: bool) -> Self {
        if value { Self::Truncated } else { Self::Complete }
    }
}

impl From<FileResultTruncation> for bool {
    fn from(value: FileResultTruncation) -> Self {
        value.is_truncated()
    }
}
