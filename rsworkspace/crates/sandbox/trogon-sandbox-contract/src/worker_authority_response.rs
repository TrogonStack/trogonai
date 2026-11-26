use serde::{Deserialize, Serialize};

use crate::{LeaseId, OwnedLease};

/// What the broker says about the leases a worker asked about. A lease in neither list is unknown to the broker.
///
/// `not_assigned` defaults to empty when absent.
#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerAuthorityResponse {
    /// Leases still assigned to the asker.
    pub owned: Vec<OwnedLease>,
    /// Leases the broker recorded but assigned to another worker, or finished.
    #[serde(default)]
    pub not_assigned: Vec<LeaseId>,
}
