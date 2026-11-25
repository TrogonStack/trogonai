use serde::{Deserialize, Serialize};

use crate::TokenId;

/// Why the credential broker refused a request, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialError {
    #[error("credential not found: {message}")]
    NotFound { message: String },
    #[error("token {token_id} expired")]
    TokenExpired { token_id: TokenId },
    #[error("token {token_id} was revoked")]
    TokenRevoked { token_id: TokenId },
    #[error("credential scope denied: {message}")]
    ScopeDenied { message: String },
    /// The request's lease and policy revision do not match a committed policy that names the reference, or a
    /// required fence is missing.
    #[error("policy fence refused: {message}")]
    PolicyFenceRefused { message: String },
    #[error("credential backend failed: {message}")]
    Backend { message: String },
}
