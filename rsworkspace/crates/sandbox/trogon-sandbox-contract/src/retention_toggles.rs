use serde::{Deserialize, Serialize};

use crate::{MonitoringRetention, SnapshotCaptureAccess};

/// Per-lease opt-outs for data retention.
///
/// A lease that handles material its owner must not leave behind turns these off at creation. Both default to on,
/// which keeps older serialized specs valid, and both fields are always written to the wire.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RetentionToggles {
    #[serde(default)]
    pub monitoring: MonitoringRetention,
    #[serde(default)]
    pub snapshots: SnapshotCaptureAccess,
}

impl RetentionToggles {
    /// Reports whether both toggles are on, so the wire can leave the group out.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
