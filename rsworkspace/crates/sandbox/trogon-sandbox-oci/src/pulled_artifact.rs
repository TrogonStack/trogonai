use trogon_sandbox_contract::OciArtifactRef;

use crate::{Annotations, Layer, ManifestSummary};

/// An artifact whose manifest and every layer matched their digests.
#[derive(Debug, Clone)]
pub struct PulledArtifact {
    pub reference: OciArtifactRef,
    pub layers: Vec<Layer>,
    pub annotations: Annotations,
    pub manifest: ManifestSummary,
}
