use serde::{Deserialize, Serialize};

use crate::{LeaseId, SnapshotPushTarget};

/// Broker-to-worker request to checkpoint a still-running lease without stopping it, so the resulting snapshot can
/// seed an independent new lease.
///
/// It is shaped like [`SnapshotExportRequest`](crate::SnapshotExportRequest) but goes to every worker for the lease's
/// provider. Only the worker actually holding `lease_id` replies, the same routing used for lease cancellation.
/// `native_name` is omitted when absent. When set, the worker takes a provider-native snapshot under that name and
/// `target` is unused.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRunningRequest {
    pub lease_id: LeaseId,
    pub target: SnapshotPushTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_name: Option<String>,
}
