use thiserror::Error;
use trogon_sandbox_contract::EnvDigest;

/// An environment policy refused to run or restore.
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum EnvironmentError {
    /// The worker's own environment is outside the approved set.
    #[error("environment {own} is not in the approved set of this node")]
    NotApproved { own: EnvDigest },
    /// The snapshot was taken in a different environment.
    #[error("snapshot was taken in environment {snapshot}, this worker runs {own}")]
    Mismatch { snapshot: EnvDigest, own: EnvDigest },
    /// The snapshot records no environment digest and the policy refuses unlabeled snapshots.
    #[error("snapshot records no environment digest and this worker refuses unlabeled snapshots")]
    LegacyRefused,
}
