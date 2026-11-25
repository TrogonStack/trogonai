use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseSnapshotReceipt, NativeSnapshotReceipt};

/// Outcome of a [`SnapshotLeaseRequest`](crate::SnapshotLeaseRequest), tagged by `outcome`.
///
/// `Unsupported` means the lease's provider cannot checkpoint a running lease.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SnapshotLeaseResponse {
    Created {
        receipt: LeaseSnapshotReceipt,
    },
    /// The provider holds the snapshot itself, and only a worker of that provider can restore it. Sent only to a caller
    /// that set `accept_native`.
    CreatedNative {
        receipt: NativeSnapshotReceipt,
    },
    Unsupported,
    Denied(LeaseError),
}
