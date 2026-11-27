use crate::resources::{ActorResourcesError, ImageDigestError};

/// Why a configuration, or a resource request against it, is unusable.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SubstrateConfigError {
    #[error("memory exceeds every configured substrate size class")]
    MemoryExceedsSizeClasses,
    #[error("agent port must be nonzero")]
    AgentPortZero,
    #[error(transparent)]
    Resources(#[from] ActorResourcesError),
    #[error(transparent)]
    Image(#[from] ImageDigestError),
}
