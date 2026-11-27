use crate::api::ApiError;

/// Why a native lifetime cannot back a lease.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum LeaseLifetimeError {
    #[error("native lifetime is missing or invalid")]
    Invalid,
    #[error("native lifetime is expired")]
    Expired,
    #[error("native lifetime is out of range")]
    OutOfRange,
    #[error(transparent)]
    Unusable(#[from] ApiError),
}
