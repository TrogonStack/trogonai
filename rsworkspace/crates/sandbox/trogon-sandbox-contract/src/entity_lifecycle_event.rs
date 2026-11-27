use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{Digest, LeaseId, SandboxEntityId, TenantId};

/// What the entity event stream carries, tagged by `event`.
///
/// It is published best effort after the record change it describes, so the entity record stays the source of
/// truth.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EntityLifecycleEvent {
    Created {
        entity_id: SandboxEntityId,
        tenant: TenantId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        at: SystemTime,
    },
    RunStarted {
        entity_id: SandboxEntityId,
        tenant: TenantId,
        lease: LeaseId,
        generation: u64,
        /// The snapshot the run restored from, when it restored from one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_digest: Option<Digest>,
        at: SystemTime,
    },
    RunEnded {
        entity_id: SandboxEntityId,
        tenant: TenantId,
        lease: LeaseId,
        /// The generation of the run that ended, so a late event sorts before the next `run_started`.
        generation: u64,
        reason: String,
        /// The snapshot the run left under the persistence tag. Absent when no capture landed before the broker
        /// stopped waiting for it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        captured_digest: Option<Digest>,
        at: SystemTime,
    },
    Deleted {
        entity_id: SandboxEntityId,
        tenant: TenantId,
        at: SystemTime,
    },
}

impl EntityLifecycleEvent {
    pub fn entity_id(&self) -> &SandboxEntityId {
        match self {
            Self::Created { entity_id, .. }
            | Self::RunStarted { entity_id, .. }
            | Self::RunEnded { entity_id, .. }
            | Self::Deleted { entity_id, .. } => entity_id,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        match self {
            Self::Created { tenant, .. }
            | Self::RunStarted { tenant, .. }
            | Self::RunEnded { tenant, .. }
            | Self::Deleted { tenant, .. } => tenant,
        }
    }
}
