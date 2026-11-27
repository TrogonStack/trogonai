use async_trait::async_trait;
use trogon_sandbox_contract::OciArtifactRef;

use super::artifact_delete_error::ArtifactDeleteError;

/// Deletes an artifact from the registry.
#[async_trait]
pub trait ArtifactDeleter: Send + Sync {
    async fn delete(&self, artifact: &OciArtifactRef) -> Result<(), ArtifactDeleteError>;
}

#[async_trait]
impl ArtifactDeleter for trogon_sandbox_oci::OciClient {
    async fn delete(&self, artifact: &OciArtifactRef) -> Result<(), ArtifactDeleteError> {
        self.delete_artifact(artifact).await?;
        Ok(())
    }
}
