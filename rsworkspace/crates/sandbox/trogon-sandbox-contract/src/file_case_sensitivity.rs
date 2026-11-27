use serde::{Deserialize, Serialize};

/// Whether a content search ignores letter case.
///
/// It serializes as the JSON boolean `case_insensitive`, `true` for `Insensitive`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileCaseSensitivity {
    #[default]
    Sensitive,
    Insensitive,
}

impl FileCaseSensitivity {
    pub const fn ignores_case(self) -> bool {
        matches!(self, Self::Insensitive)
    }
}

impl From<bool> for FileCaseSensitivity {
    fn from(value: bool) -> Self {
        if value { Self::Insensitive } else { Self::Sensitive }
    }
}

impl From<FileCaseSensitivity> for bool {
    fn from(value: FileCaseSensitivity) -> Self {
        value.ignores_case()
    }
}
