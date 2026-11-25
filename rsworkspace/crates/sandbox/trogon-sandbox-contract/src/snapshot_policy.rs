use serde::{Deserialize, Serialize};

use crate::{ArtifactType, PrewarmSpec, RetentionPolicy};

/// Where a lease's snapshots go and how they are kept.
///
/// The optional fields are omitted from JSON when absent and default to `None` when an older peer
/// leaves them out.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SnapshotPolicy {
    pub artifact_type: ArtifactType,
    pub target_repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<RetentionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prewarm: Option<PrewarmSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_name: Option<String>,
}
