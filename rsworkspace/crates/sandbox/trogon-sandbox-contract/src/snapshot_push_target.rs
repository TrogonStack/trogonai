use serde::{Deserialize, Serialize};

use crate::ArtifactType;

/// Push destination for a worker-side snapshot export.
///
/// The digest is assigned at push time.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPushTarget {
    pub registry: String,
    pub repository: String,
    pub artifact_type: ArtifactType,
}
