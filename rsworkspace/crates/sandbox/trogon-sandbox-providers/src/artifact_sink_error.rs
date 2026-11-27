/// Why an artifact sink could not store or return an artifact.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactSinkError {
    #[error("artifact sink does not support get: {object_key}")]
    GetUnsupported { object_key: String },
    #[error("artifact sink failed: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}
