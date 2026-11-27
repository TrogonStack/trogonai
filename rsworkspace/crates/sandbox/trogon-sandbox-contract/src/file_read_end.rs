use serde::{Deserialize, Serialize};

/// Whether a read reached the end of the file.
///
/// It serializes as the JSON boolean `eof`, `true` for `End`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileReadEnd {
    #[default]
    More,
    End,
}

impl FileReadEnd {
    pub const fn reached_end(self) -> bool {
        matches!(self, Self::End)
    }
}

impl From<bool> for FileReadEnd {
    fn from(value: bool) -> Self {
        if value { Self::End } else { Self::More }
    }
}

impl From<FileReadEnd> for bool {
    fn from(value: FileReadEnd) -> Self {
        value.reached_end()
    }
}
