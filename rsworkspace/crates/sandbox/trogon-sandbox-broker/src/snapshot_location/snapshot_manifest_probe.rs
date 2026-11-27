use async_trait::async_trait;
use tracing::debug;
use trogon_sandbox_contract::OciArtifactRef;

/// Answers whether a registry holds a manifest.
#[async_trait]
pub trait SnapshotManifestProbe: Send + Sync {
    async fn exists(&self, artifact: &OciArtifactRef) -> bool;
}

#[async_trait]
impl SnapshotManifestProbe for trogon_sandbox_oci::OciClient {
    async fn exists(&self, artifact: &OciArtifactRef) -> bool {
        match self.head_manifest(artifact).await {
            Ok(_) => true,
            Err(e) => {
                debug!(
                    repository = %artifact.repository,
                    digest = %artifact.digest,
                    error = %e,
                    "snapshot manifest probe missed"
                );
                false
            }
        }
    }
}
