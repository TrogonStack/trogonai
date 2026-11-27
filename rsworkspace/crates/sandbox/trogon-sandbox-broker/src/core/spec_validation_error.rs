/// Why a lease request's lifetime fields are unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum SpecValidationError {
    #[error("ttl_seconds must be > 0")]
    ZeroTtl,
    #[error("idle_timeout_seconds must be between 1 and {max_lifetime_seconds}")]
    IdleTimeoutTooLong { max_lifetime_seconds: u32 },
}
