use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::OciError;
use trogon_sandbox_providers::K8sProviderError;

/// Failures moving a rootfs tar between a pod and the registry.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotTransferError {
    #[error("tar workspace dir")]
    Tar(#[source] std::io::Error),
    #[error("export workspace via kubernetes exec")]
    Export(#[source] K8sProviderError),
    #[error("build push target")]
    BuildPushTarget(#[source] OciError),
    #[error("push artifact")]
    Push(#[source] OciError),
    #[error("pull snapshot artifact")]
    Pull(#[source] OciError),
    #[error("unsupported snapshot artifact type {found:?}, expected SandboxRootfsV1")]
    UnsupportedArtifactType { found: ArtifactType },
}
