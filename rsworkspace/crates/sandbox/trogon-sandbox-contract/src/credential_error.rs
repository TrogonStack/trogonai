use serde::{Deserialize, Serialize};

use crate::TokenId;

/// Why the credential broker refused a request, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialError {
    NotFound {
        message: String,
    },
    TokenExpired {
        token_id: TokenId,
    },
    TokenRevoked {
        token_id: TokenId,
    },
    ScopeDenied {
        message: String,
    },
    /// The request's lease and policy revision do not match a committed policy that names the reference, or a
    /// required fence is missing.
    PolicyFenceRefused {
        message: String,
    },
    Backend {
        message: String,
    },
}
