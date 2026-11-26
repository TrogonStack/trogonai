use trogon_sandbox_contract::TenantIdError;

/// A stored API key record does not satisfy the rules [`ApiKeyRecord`](crate::ApiKeyRecord) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ApiKeyRecordError {
    #[error("API key record tenant is invalid: {0}")]
    Tenant(#[from] TenantIdError),
    #[error("API key record creation time is out of range")]
    CreatedAtOutOfRange,
}
