use serde::{Deserialize, Serialize};

/// Whether a named lease resumes from, and captures to, a snapshot tag derived from its name.
///
/// It serializes as a JSON boolean named `persistent`, `true` for `Persistent`. An omitted value reads as
/// `Persistent`. The flag has no effect on a lease without a name.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LeasePersistence {
    Ephemeral,
    #[default]
    Persistent,
}

impl LeasePersistence {
    pub const fn is_persistent(self) -> bool {
        matches!(self, Self::Persistent)
    }
}

impl From<bool> for LeasePersistence {
    fn from(flag: bool) -> Self {
        if flag { Self::Persistent } else { Self::Ephemeral }
    }
}

impl From<LeasePersistence> for bool {
    fn from(value: LeasePersistence) -> Self {
        value.is_persistent()
    }
}
