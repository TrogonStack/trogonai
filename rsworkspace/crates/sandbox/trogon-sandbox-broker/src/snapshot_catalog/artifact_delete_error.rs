/// Why deleting a registry artifact failed.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactDeleteError {
    #[error("registry artifact delete failed: {0}")]
    Registry(#[from] trogon_sandbox_oci::OciError),
}
