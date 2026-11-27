use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    DriveMount, EntityState, LeaseId, SandboxEntityId, SandboxSpec, SandboxTags, ServiceAccountId, SnapshotHead,
    SnapshotTag, TenantId,
};

/// The durable record of a sandbox, keyed by its entity id within a tenant. It outlives any single lease.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxEntityRecord {
    pub entity_id: SandboxEntityId,
    pub tenant: TenantId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created_at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ServiceAccountId>,
    /// The template for the next start. A run's actual spec is on its lease.
    pub spec: SandboxSpec,
    #[serde(default)]
    pub metadata: SandboxTags,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<DriveMount>,
    pub persistence_tag: SnapshotTag,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<SnapshotHead>,
    pub state: EntityState,
    /// Grows by one on every start.
    #[serde(default)]
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_lease: Option<LeaseId>,
}
