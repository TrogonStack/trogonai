use serde::{Deserialize, Serialize};

use crate::{LeaseEpoch, LeaseId};

/// A lease the broker still assigns to the asking worker.
///
/// `epoch` is the epoch of the current assignment. It is omitted when absent, and a record written before epochs
/// existed carries none.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OwnedLease {
    pub lease_id: LeaseId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<LeaseEpoch>,
}
