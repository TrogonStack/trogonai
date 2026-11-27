use serde::{Deserialize, Serialize};

/// Whether the platform may capture the disk of a lease.
///
/// Captures are `snapshot_out`, named automatic persistence, running checkpoints, and forks. With `Refused`, each of
/// them is refused, so no copy of the sandbox disk is ever written. It serializes as a JSON boolean, `true` for
/// `Allowed`. An omitted value reads as `Allowed`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum SnapshotCaptureAccess {
    #[default]
    Allowed,
    Refused,
}

impl SnapshotCaptureAccess {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl From<bool> for SnapshotCaptureAccess {
    fn from(allowed: bool) -> Self {
        if allowed { Self::Allowed } else { Self::Refused }
    }
}

impl From<SnapshotCaptureAccess> for bool {
    fn from(access: SnapshotCaptureAccess) -> Self {
        access.is_allowed()
    }
}
