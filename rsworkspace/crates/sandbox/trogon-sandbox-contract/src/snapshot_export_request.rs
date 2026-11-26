use serde::{Deserialize, Serialize};

use crate::{LeaseId, SnapshotPushTarget};

/// Request for a worker to export a lease's snapshot to a push target.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotExportRequest {
    pub lease_id: LeaseId,
    pub target: SnapshotPushTarget,
}
