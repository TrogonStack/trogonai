use async_trait::async_trait;
use tracing::debug;
use trogon_sandbox_contract::{ArtifactType, OciArtifactRef};

use super::registry_inventory_error::RegistryInventoryError;

/// What the lister needs from a registry.
#[async_trait]
pub trait SnapshotRegistryInventory: Send + Sync {
    async fn tags(&self, registry: &str, repository: &str) -> Result<Vec<String>, RegistryInventoryError>;

    /// Artifact type of the manifest, or `None` when it is missing or not a
    /// type this system publishes.
    async fn artifact_type(&self, artifact: &OciArtifactRef) -> Option<ArtifactType>;
}

#[async_trait]
impl SnapshotRegistryInventory for trogon_sandbox_oci::OciClient {
    async fn tags(&self, registry: &str, repository: &str) -> Result<Vec<String>, RegistryInventoryError> {
        let registry = trogon_sandbox_oci::RegistryHost::new(registry)?;
        Ok(self.list_tags(&registry, repository).await?)
    }

    async fn artifact_type(&self, artifact: &OciArtifactRef) -> Option<ArtifactType> {
        // An unknown type falls back to the SBOM type, which is not a snapshot.
        let probe = OciArtifactRef {
            artifact_type: ArtifactType::SbomV1,
            ..artifact.clone()
        };
        match self.head_manifest(&probe).await {
            Ok(summary) => Some(summary.artifact_type),
            Err(e) => {
                debug!(
                    repository = %artifact.repository,
                    digest = %artifact.digest,
                    error = %e,
                    "artifact lister: manifest probe missed"
                );
                None
            }
        }
    }
}
