use trogon_sandbox_contract::{FileSha256Error, LeaseEpochError, TenantIdError, TokenError};

/// Why a dispatch could not be signed.
#[derive(Debug, thiserror::Error)]
pub enum DispatchSignError {
    /// The payload is not JSON.
    #[error("dispatch is not JSON: {0}")]
    NotJson(serde_json::Error),
    /// The dispatch names no request tenant.
    #[error("dispatch has no request tenant")]
    MissingTenant,
    /// The dispatch names no lease id.
    #[error("dispatch has no lease id")]
    MissingLease,
    /// The dispatch is not a JSON object.
    #[error("dispatch is not an object")]
    NotObject,
    /// The request tenant is not a valid tenant id.
    #[error("dispatch request tenant is invalid: {0}")]
    InvalidTenant(TenantIdError),
    /// The target worker is not a valid worker id.
    #[error("dispatch target worker is invalid: {0}")]
    InvalidWorker(TokenError),
    /// The target attempt is not a valid attempt id.
    #[error("dispatch target attempt is invalid: {0}")]
    InvalidAttempt(TokenError),
    /// The target epoch is zero.
    #[error("dispatch target epoch is invalid: {0}")]
    InvalidEpoch(LeaseEpochError),
    /// The body digest is not a valid checksum.
    #[error("dispatch body digest is invalid: {0}")]
    Digest(FileSha256Error),
    /// The signed dispatch could not be serialized.
    #[error("signed dispatch could not be encoded: {0}")]
    Encode(serde_json::Error),
}
