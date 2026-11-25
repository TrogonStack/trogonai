use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ArtifactType, SnapshotGeneration, TenantId};

/// A snapshot the tenant is currently charged for.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LiveSnapshotEntry {
    pub tenant: TenantId,
    pub artifact_type: ArtifactType,
    pub published_at: SystemTime,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<SnapshotGeneration>,
}
