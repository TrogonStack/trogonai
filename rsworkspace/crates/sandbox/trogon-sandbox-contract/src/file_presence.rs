use serde::{Deserialize, Serialize};

/// Whether the entry or watch the operation targeted existed beforehand.
///
/// It serializes as the JSON boolean `existed`, `true` for `Present`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FilePresence {
    #[default]
    Absent,
    Present,
}

impl FilePresence {
    pub const fn was_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

impl From<bool> for FilePresence {
    fn from(value: bool) -> Self {
        if value { Self::Present } else { Self::Absent }
    }
}

impl From<FilePresence> for bool {
    fn from(value: FilePresence) -> Self {
        value.was_present()
    }
}
