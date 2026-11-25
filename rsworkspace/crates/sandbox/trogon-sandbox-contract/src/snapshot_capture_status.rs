use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest};

/// The outcome of a lease's snapshot capture, tagged by `kind`, so a failed capture can be looked
/// up per lease.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SnapshotCaptureStatus {
    Created {
        digest: Digest,
        artifact_type: ArtifactType,
    },
    Failed {
        reason: String,
    },
}
