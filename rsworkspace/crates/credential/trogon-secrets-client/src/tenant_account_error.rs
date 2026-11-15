/// Why a value does not name a tenant account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TenantAccountError {
    #[error("tenant account is not an NKey public key")]
    NotAPublicKey,
    #[error("tenant account is an NKey public key that names something other than an account")]
    NotAnAccount,
}
