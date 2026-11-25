use serde::{Deserialize, Serialize};

use crate::LeaseId;

/// The committed policy a credential request was built from.
///
/// `lease_id` names the lease that owns the policy and `policy_revision` the exact committed revision. The broker
/// compares both with what the control plane committed and refuses a request whose revision is stale or ahead, or
/// whose reference that policy does not name.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PolicyFence {
    pub lease_id: LeaseId,
    pub policy_revision: u64,
}
