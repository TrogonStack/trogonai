use crate::{GitStep, VolumeCacheError};

/// Failures of the subprocess provider.
#[derive(Debug, thiserror::Error)]
pub enum SubprocessError {
    #[error("host_mount workspaces are not supported on the subprocess provider")]
    HostMountUnsupported,
    #[error("NatsKv SecretRef is not supported in SubprocessSandboxProvider; use Literal for local testing")]
    NatsKvSecretUnsupported,
    #[error("clone_url must contain ://")]
    CloneUrlMissingScheme,
    #[error("git {step} failed")]
    GitUnavailable {
        step: GitStep,
        #[source]
        source: std::io::Error,
    },
    #[error("git {step} failed: {stderr}")]
    GitFailed { step: GitStep, stderr: String },
    #[error("failed to spawn the agent process: {0}")]
    Spawn(#[source] std::io::Error),
    #[error(transparent)]
    VolumeCache(#[from] VolumeCacheError),
}
