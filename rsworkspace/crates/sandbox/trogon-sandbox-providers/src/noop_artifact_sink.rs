use crate::{ArtifactRef, ArtifactSink, ArtifactSinkError};

/// A sink that records nothing: `put` echoes a reference and `get` is unsupported.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopArtifactSink;

#[async_trait::async_trait]
impl ArtifactSink for NoopArtifactSink {
    async fn put(
        &self,
        tool: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactRef, ArtifactSinkError> {
        Ok(ArtifactRef {
            tool: tool.to_string(),
            object_bucket: "noop".to_string(),
            object_key: key.to_string(),
            sha256: String::new(),
            size: bytes.len() as u64,
            content_type: content_type.to_string(),
        })
    }

    async fn get(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactSinkError> {
        Err(ArtifactSinkError::GetUnsupported {
            object_key: artifact.object_key.clone(),
        })
    }
}
