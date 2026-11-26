use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest};

/// An artifact pinned by digest in an OCI registry.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct OciArtifactRef {
    pub registry: String,
    pub repository: String,
    pub digest: Digest,
    pub artifact_type: ArtifactType,
}

impl OciArtifactRef {
    /// The reference as `registry/repository@digest`.
    pub fn allowlist_string(&self) -> String {
        format!("{}/{}@{}", self.registry, self.repository, self.digest)
    }
}
