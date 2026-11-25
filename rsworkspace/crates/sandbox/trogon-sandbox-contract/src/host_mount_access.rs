use serde::{Deserialize, Serialize};

/// Whether a lease may write to a host directory mounted as its workspace.
///
/// It serializes as the JSON boolean `readonly`, `true` for `ReadOnly`, so existing peers keep
/// parsing it. An omitted value reads as `ReadWrite`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum HostMountAccess {
    #[default]
    ReadWrite,
    ReadOnly,
}

impl HostMountAccess {
    pub const fn is_read_only(self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

impl From<bool> for HostMountAccess {
    fn from(readonly: bool) -> Self {
        if readonly { Self::ReadOnly } else { Self::ReadWrite }
    }
}

impl From<HostMountAccess> for bool {
    fn from(access: HostMountAccess) -> Self {
        access.is_read_only()
    }
}
