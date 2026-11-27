use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    DriveMount, EntityState, LeaseId, SandboxEntityId, SandboxEntityRecord, SandboxTags, SnapshotHead, TenantId,
};

/// What a client may read of a [`SandboxEntityRecord`].
///
/// The spec template is left out because it can carry environment values and only matters to the broker when it
/// starts the next run.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxEntityRow {
    pub entity_id: SandboxEntityId,
    pub tenant: TenantId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created_at: SystemTime,
    #[serde(default)]
    pub metadata: SandboxTags,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<DriveMount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<SnapshotHead>,
    pub state: EntityState,
    #[serde(default)]
    pub generation: u64,
    /// The lease currently running this entity, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_lease: Option<LeaseId>,
}

impl From<&SandboxEntityRecord> for SandboxEntityRow {
    fn from(record: &SandboxEntityRecord) -> Self {
        Self {
            entity_id: record.entity_id.clone(),
            tenant: record.tenant.clone(),
            name: record.name.clone(),
            created_at: record.created_at,
            metadata: record.metadata.clone(),
            drives: record.drives.clone(),
            head: record.head.clone(),
            state: record.state.clone(),
            generation: record.generation,
            current_lease: record.current_lease.clone(),
        }
    }
}
