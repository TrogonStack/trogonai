use super::evolve_error::EvolveError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeartbeatError {
    #[error("peer identity is not enrolled")]
    MissingEnrollment,
    #[error("authenticated owner does not own this peer")]
    OwnerMismatch,
    #[error("connector attachment has been fenced")]
    Fenced,
    #[error("connector attachment is inactive")]
    Inactive,
    #[error("connector attachment lease expired")]
    Expired,
    #[error("service time moved backwards")]
    ClockRegression,
    #[error("reported activity moved backwards or is in the future")]
    InvalidActivity,
    #[error("attachment deadline is outside the supported range")]
    LeaseDeadlineOverflow,
    #[error("invalid peer state: {0}")]
    InvalidState(#[from] EvolveError),
}
