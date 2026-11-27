use crate::{ArtifactRef, ArtifactSinkError};

/// Where a sandbox's tools put the artifacts they produce.
#[async_trait::async_trait]
pub trait ArtifactSink: Send + Sync {
    async fn put(
        &self,
        tool: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactRef, ArtifactSinkError>;

    async fn get(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactSinkError>;
}
