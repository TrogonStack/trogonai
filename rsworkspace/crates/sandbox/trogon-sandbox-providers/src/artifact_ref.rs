use serde::{Deserialize, Serialize};

/// A stored artifact a tool produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub tool: String,
    pub object_bucket: String,
    pub object_key: String,
    pub sha256: String,
    pub size: u64,
    pub content_type: String,
}
