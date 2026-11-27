use trogon_sandbox_auth::{BearerToken, BearerTokenError};

use crate::EntropyError;
use crate::constants::SESSION_TOKEN_BYTES;
use crate::random_hex::random_hex;

/// A fresh unguessable session token.
pub fn mint_session_token() -> Result<String, EntropyError> {
    random_hex(SESSION_TOKEN_BYTES)
}

/// The hash a session token is stored under.
pub fn session_token_hash(token: &str) -> Result<String, BearerTokenError> {
    Ok(BearerToken::new(token)?.hash().as_str().to_string())
}
