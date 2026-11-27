use serde::{Deserialize, Serialize};

use crate::{LeaseId, NetworkPolicy};

/// Request to replace the egress network policy of a running lease in place, without restarting or
/// re-provisioning the sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkPolicyUpdateRequest {
    pub lease_id: LeaseId,
    pub network_policy: NetworkPolicy,
}
