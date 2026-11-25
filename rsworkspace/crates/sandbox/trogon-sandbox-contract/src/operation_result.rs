use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseGranted, LeaseSnapshotReceipt, SuspendLevel};

/// The successful payload of an operation, tagged by `result`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum OperationResult {
    Granted(LeaseGranted),
    Snapshot(LeaseSnapshotReceipt),
    Paused {
        reap_at: SystemTime,
    },
    Resumed {
        deadline: SystemTime,
    },
    Suspended {
        level: SuspendLevel,
        reap_at: SystemTime,
    },
    Woken {
        deadline: SystemTime,
    },
    Stopped,
    Deleted,
    RolledBack,
    /// The provider cannot perform the verb.
    Unsupported,
}
