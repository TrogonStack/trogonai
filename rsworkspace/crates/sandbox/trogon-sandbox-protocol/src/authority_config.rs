use std::time::Duration;

use crate::AuthorityMode;
use crate::constants::{
    CONFIRMATIONS, DEFAULT_CHECK_INTERVAL, DEFAULT_GRACE, DEFAULT_PARTITION_AFTER, DEFAULT_REQUEST_TIMEOUT,
    MIN_LEASE_AGE,
};

/// Timing and thresholds of the authority guard.
#[derive(Debug, Clone, Copy)]
pub struct AuthorityConfig {
    /// How far the guard goes.
    pub mode: AuthorityMode,
    /// Time between checks.
    pub interval: Duration,
    /// Failed checks for this long make every held lease suspect.
    pub partition_after: Duration,
    /// Time between freezing a suspect lease and purging it.
    pub grace: Duration,
    /// A lease younger than this is never judged.
    pub min_lease_age: Duration,
    /// Consecutive checks in which the broker must disown a lease.
    pub confirmations: u32,
    /// How long one request to the broker may take.
    pub request_timeout: Duration,
}

impl AuthorityConfig {
    /// The default timing for `mode`.
    pub fn new(mode: AuthorityMode) -> Self {
        Self {
            mode,
            interval: DEFAULT_CHECK_INTERVAL,
            partition_after: DEFAULT_PARTITION_AFTER,
            grace: DEFAULT_GRACE,
            min_lease_age: MIN_LEASE_AGE,
            confirmations: CONFIRMATIONS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}
