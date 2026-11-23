use serde::{Deserialize, Serialize};

use crate::{ArtifactType, CapabilitySupport};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotCapability {
    pub can_snapshot: CapabilitySupport,
    pub can_checkpoint_running: CapabilitySupport,
    pub can_restore: CapabilitySupport,
    pub supported_artifact_types: Vec<ArtifactType>,
    pub layered_deltas: CapabilitySupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_snapshot_size_bytes: Option<u64>,
    #[serde(default)]
    pub supports_native_snapshots: CapabilitySupport,
    #[serde(default)]
    pub supports_in_place_rollback: CapabilitySupport,
}
