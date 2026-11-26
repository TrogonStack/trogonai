use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest};

/// One node in a [`SnapshotTreeResponse`](crate::SnapshotTreeResponse).
///
/// `parent` is omitted for a root snapshot with no layered-delta base. `created_at` uses serde's `SystemTime`
/// encoding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTreeNode {
    pub digest: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Digest>,
    pub artifact_type: ArtifactType,
    pub created_at: SystemTime,
}
