use serde::{Deserialize, Serialize};

/// Whether a checkpoint fingerprints file content or only size and modification time.
///
/// It serializes as the JSON boolean `hash`, `true` for `Content`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileContentHashing {
    #[default]
    Metadata,
    Content,
}

impl FileContentHashing {
    pub const fn hashes_content(self) -> bool {
        matches!(self, Self::Content)
    }
}

impl From<bool> for FileContentHashing {
    fn from(value: bool) -> Self {
        if value { Self::Content } else { Self::Metadata }
    }
}

impl From<FileContentHashing> for bool {
    fn from(value: FileContentHashing) -> Self {
        value.hashes_content()
    }
}
