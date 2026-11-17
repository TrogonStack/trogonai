use std::time::Duration;

/// TypeScript `QI`: reconnects in a row before the transport gives up.
pub(crate) const RECONNECT_BUDGET: u32 = 5;

/// TypeScript `n4`: how long the stream request may take to answer.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_millis(15_000);

/// TypeScript `r4`: how long the stream may stay silent.
pub(crate) const LIVENESS_TIMEOUT: Duration = Duration::from_millis(45_000);

/// TypeScript `o4`: how long a write POST may take.
pub(crate) const POST_TIMEOUT: Duration = Duration::from_millis(30_000);

pub(super) const BACKOFF_BASE_MS: u64 = 1_000;

pub(super) const BACKOFF_CAP_MS: u64 = 30_000;

pub(super) const JITTER_US: u64 = 250_000;
