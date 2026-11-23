use serde::{Deserialize, Serialize};

use crate::ArtifactType;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotCapability {
    pub can_snapshot: bool,
    pub can_checkpoint_running: bool,
    pub can_restore: bool,
    pub supported_artifact_types: Vec<ArtifactType>,
    pub layered_deltas: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_snapshot_size_bytes: Option<u64>,
    #[serde(default)]
    pub supports_native_snapshots: bool,
    #[serde(default)]
    pub supports_in_place_rollback: bool,
}
