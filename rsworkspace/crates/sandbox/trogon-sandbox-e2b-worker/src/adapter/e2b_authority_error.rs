use trogon_sandbox_protocol::{AuthorityHostError, AuthorityResourceIdError};

use crate::api::ApiError;

/// Why the authority guard could not act on a lease this worker holds.
#[derive(Debug, thiserror::Error)]
pub enum E2bAuthorityError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("E2B named the sandbox with an id the authority guard refuses: {0}")]
    ResourceId(#[from] AuthorityResourceIdError),
    #[error("lease is held only for cleanup")]
    CleanupOnly,
    #[error("E2B did not confirm a running or paused sandbox")]
    StateUnconfirmed,
    #[error("E2B pause conflict did not confirm an already-paused sandbox")]
    PauseConflictUnconfirmed,
}

impl From<E2bAuthorityError> for AuthorityHostError {
    fn from(error: E2bAuthorityError) -> Self {
        Self::Failed(Box::new(error))
    }
}
