use async_trait::async_trait;

use super::artifact_list_error::ArtifactListError;
use super::listed_artifact::ListedArtifact;

/// Source of the artifacts that exist in the registry. The OCI client has no
/// listing call, so a deployment supplies one (a registry catalog walk, or an
/// inventory its registry exports).
#[async_trait]
pub trait ArtifactLister: Send + Sync {
    async fn list(&self) -> Result<Vec<ListedArtifact>, ArtifactListError>;
}
