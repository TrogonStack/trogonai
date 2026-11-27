use serde::{Deserialize, Serialize};

/// Whether missing parent directories are created along the way.
///
/// It serializes as the JSON boolean `parents`, `true` for `Create`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileParentCreation {
    Require,
    #[default]
    Create,
}

impl FileParentCreation {
    pub const fn creates(self) -> bool {
        matches!(self, Self::Create)
    }
}

impl From<bool> for FileParentCreation {
    fn from(value: bool) -> Self {
        if value { Self::Create } else { Self::Require }
    }
}

impl From<FileParentCreation> for bool {
    fn from(value: FileParentCreation) -> Self {
        value.creates()
    }
}
