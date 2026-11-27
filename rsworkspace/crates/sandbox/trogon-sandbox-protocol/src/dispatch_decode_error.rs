use crate::DispatchAuthError;

/// What [`DispatchVerifier::decode`](crate::DispatchVerifier::decode) returns when the dispatch must not proceed.
#[derive(Debug, thiserror::Error)]
pub enum DispatchDecodeError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Auth(#[from] DispatchAuthError),
}
