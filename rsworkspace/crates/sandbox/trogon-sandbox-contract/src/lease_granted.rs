use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{LeaseCreation, LeaseId, OperationId, ProviderKind, SandboxEntityId};

/// The broker's grant of a lease.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseGranted {
    pub lease_id: LeaseId,
    pub provider: ProviderKind,
    /// The opaque identifier the provider assigned to the sandbox.
    pub sandbox_id: String,
    pub evidence_subject: String,
    pub deadline: SystemTime,
    /// Whether this response created the lease. Serialized as a boolean named `created`.
    #[serde(default)]
    pub created: LeaseCreation,
    /// Optional SDK features the lease's provider supports, named as the client's feature list. Empty when the
    /// broker had no live profile for the provider, which clients read as unknown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// The persistent entity this lease is a run of. Absent when the broker does not track entities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<SandboxEntityId>,
    /// The durable operation that created this lease, so a caller that sent an operation id can poll it. Absent
    /// for a lease made without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
}
