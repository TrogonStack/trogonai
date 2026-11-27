use trogon_sandbox_auth::BearerTokenError;

use crate::{PreviewTokenError, SessionStoreError};

/// Why a preview grant could not be made or stored.
#[derive(Debug, thiserror::Error)]
pub enum PreviewGrantError {
    #[error("preview token could not be signed: {0}")]
    Token(#[from] PreviewTokenError),
    #[error("preview grant key is invalid: {0}")]
    Key(#[from] BearerTokenError),
    #[error("preview grant could not be stored: {0}")]
    Store(#[from] SessionStoreError),
}
