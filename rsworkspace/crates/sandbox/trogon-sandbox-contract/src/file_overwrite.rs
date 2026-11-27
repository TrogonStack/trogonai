use serde::{Deserialize, Serialize};

/// Whether a rename may replace an existing destination.
///
/// It serializes as the JSON boolean `overwrite`, `true` for `Overwrite`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileOverwrite {
    #[default]
    Refuse,
    Overwrite,
}

impl FileOverwrite {
    pub const fn overwrites(self) -> bool {
        matches!(self, Self::Overwrite)
    }
}

impl From<bool> for FileOverwrite {
    fn from(value: bool) -> Self {
        if value { Self::Overwrite } else { Self::Refuse }
    }
}

impl From<FileOverwrite> for bool {
    fn from(value: FileOverwrite) -> Self {
        value.overwrites()
    }
}
