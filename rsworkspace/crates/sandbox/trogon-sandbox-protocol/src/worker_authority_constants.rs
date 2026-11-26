use std::time::Duration;

/// How a worker reacts to leases the broker does not list as its own.
pub const ENV_WORKER_AUTHORITY: &str = "SANDBOX_WORKER_AUTHORITY";

/// Most lease ids one request may carry; a worker splits larger sets.
pub const MAX_LEASES_PER_REQUEST: usize = 10_000;

/// Time between authority checks.
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// Failed checks for this long make every held lease suspect.
pub const DEFAULT_PARTITION_AFTER: Duration = Duration::from_secs(45);
/// Time between freezing a suspect lease and purging it.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(60);
/// A lease younger than this is never frozen on the broker's say-so.
pub const MIN_LEASE_AGE: Duration = Duration::from_secs(30);
/// Consecutive checks in which the broker must disown a lease.
pub const CONFIRMATIONS: u32 = 2;
/// How long one authority request may take.
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
