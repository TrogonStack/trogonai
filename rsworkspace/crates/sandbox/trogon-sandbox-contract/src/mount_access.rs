use serde::{Deserialize, Serialize};

/// Whether a lease may write to a directory mounted into its sandbox.
///
/// It serializes as the JSON boolean `readonly`, `true` for `ReadOnly`, so existing peers keep parsing it. An
/// omitted value reads as `ReadWrite`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum MountAccess {
    #[default]
    ReadWrite,
    ReadOnly,
}

impl MountAccess {
    pub const fn is_read_only(self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

impl From<bool> for MountAccess {
    fn from(flag: bool) -> Self {
        if flag { Self::ReadOnly } else { Self::ReadWrite }
    }
}

impl From<MountAccess> for bool {
    fn from(value: MountAccess) -> Self {
        value.is_read_only()
    }
}
