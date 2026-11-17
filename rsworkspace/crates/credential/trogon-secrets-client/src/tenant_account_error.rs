use std::sync::Arc;

/// Why a value does not name a tenant account.
#[derive(Clone, Debug, thiserror::Error)]
pub enum TenantAccountError {
    #[error("tenant account is not an NKey public key")]
    NotAPublicKey(#[source] Arc<nkeys::error::Error>),
    #[error("tenant account is an NKey public key that names something other than an account")]
    NotAnAccount,
}
