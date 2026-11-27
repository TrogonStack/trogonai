use trogon_sandbox_contract::{LeaseError, LeaseId};

/// Why a request was refused before it could touch the sandbox.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum BindingError {
    /// The actor resumed but no worker has claimed it yet.
    #[error("actor agent is not bound to a lease")]
    Unbound,
    /// The caller is talking to a lease that no longer owns this actor.
    #[error("lease {requested} does not own this actor, which serves {bound}")]
    LeaseMismatch { bound: LeaseId, requested: LeaseId },
}

impl From<BindingError> for LeaseError {
    fn from(error: BindingError) -> Self {
        match error {
            BindingError::Unbound => LeaseError::Backend {
                message: error.to_string(),
            },
            BindingError::LeaseMismatch { requested, .. } => LeaseError::LeaseNotFound { lease_id: requested },
        }
    }
}
