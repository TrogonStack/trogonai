use serde::{Deserialize, Serialize};

use crate::{LeaseId, WorkerId};

/// What a worker asks the broker: which of these leases are still mine?
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerAuthorityRequest {
    pub worker_id: WorkerId,
    pub lease_ids: Vec<LeaseId>,
}
