use serde::{Deserialize, Serialize};

/// Whether older watch events fell out of the buffer before they were polled.
///
/// It serializes as the JSON boolean `overflowed`, `true` for `Overflowed`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileWatchOverflow {
    #[default]
    Intact,
    Overflowed,
}

impl FileWatchOverflow {
    pub const fn overflowed(self) -> bool {
        matches!(self, Self::Overflowed)
    }
}

impl From<bool> for FileWatchOverflow {
    fn from(value: bool) -> Self {
        if value { Self::Overflowed } else { Self::Intact }
    }
}

impl From<FileWatchOverflow> for bool {
    fn from(value: FileWatchOverflow) -> Self {
        value.overflowed()
    }
}
