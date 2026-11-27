use crate::{ArtifactSinkError, DockerError, K8sError, LeaseSpecError, SubprocessError};

/// Why a provider could not launch or cancel a sandbox.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error(transparent)]
    Spec(#[from] LeaseSpecError),
    #[error(transparent)]
    Docker(#[from] DockerError),
    #[error(transparent)]
    Kubernetes(#[from] K8sError),
    #[error(transparent)]
    Subprocess(#[from] SubprocessError),
    #[error(transparent)]
    Artifact(#[from] ArtifactSinkError),
}
