/// A resolved container image description was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolvedContainerImageError {
    #[error("image reference is not pinned to the resolved digest")]
    NotPinned,

    #[error("image environment name is empty or contains a NUL byte")]
    InvalidEnvName,

    #[error("image environment value contains a NUL byte")]
    InvalidEnvValue,

    #[error("image working directory must be an absolute path without NUL bytes")]
    InvalidWorkingDir,
}
