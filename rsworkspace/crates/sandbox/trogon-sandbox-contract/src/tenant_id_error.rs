use crate::TenantId;

/// A new tenant identifier does not satisfy the rules [`TenantId::new`] enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TenantIdError {
    #[error("tenant id must not be empty")]
    Empty,
    #[error("tenant id must be at most {} bytes", TenantId::MAX_LEN)]
    TooLong,
    #[error("tenant id must not contain control characters")]
    ControlCharacter,
}
