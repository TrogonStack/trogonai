use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest};

/// What a provider reports after it exports a snapshot.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotReceipt {
    pub digest: Digest,
    pub bytes: u64,
    pub artifact_type: ArtifactType,
}
