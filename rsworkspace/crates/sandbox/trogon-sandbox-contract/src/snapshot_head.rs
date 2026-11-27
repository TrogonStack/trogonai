use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest, LeaseId};

/// The newest snapshot of an entity, which is the state its next run restores from.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotHead {
    pub digest: Digest,
    pub artifact_type: ArtifactType,
    pub captured_at: SystemTime,
    pub from_lease: LeaseId,
}
