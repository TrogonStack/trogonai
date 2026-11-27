use trogon_sandbox_contract::{ArtifactType, Digest};

use crate::Annotations;

/// The fields of an artifact manifest that callers act on.
#[derive(Debug, Clone)]
pub struct ManifestSummary {
    pub digest: Digest,
    pub artifact_type: ArtifactType,
    pub media_type: String,
    pub size: u64,
    pub annotations: Annotations,
}
