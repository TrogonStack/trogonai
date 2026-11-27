use trogon_sandbox_contract::CredentialError;

use crate::BackendError;

/// A token store could not issue, check or revoke a token.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    /// A stored token could not be encoded or decoded.
    #[error("token record could not be encoded or decoded")]
    Codec(#[from] serde_json::Error),
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// The token could not be validated for a reason other than being unknown, expired or revoked.
    #[error("token could not be validated: {0}")]
    Rejected(CredentialError),
}
