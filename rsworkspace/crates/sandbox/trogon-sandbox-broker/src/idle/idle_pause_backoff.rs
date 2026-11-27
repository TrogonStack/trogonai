use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::IdlePauseRetry;

/// Spaces out idle pauses that keep failing.
///
/// A provider that refuses a pause would otherwise be asked again on every
/// sweep tick for as long as the lease stays idle. Failures back off
/// exponentially from [`Self::BASE`] up to [`Self::MAX`]; a success, or a
/// lease that stops failing for long enough, starts over. The state lives on
/// the lease record (`LeaseRecord::idle_pause_retry`), so it survives a
/// broker restart.
pub struct IdlePauseBackoff;

impl IdlePauseBackoff {
    pub const BASE: Duration = Duration::from_secs(30);
    pub const MAX: Duration = Duration::from_secs(15 * 60);

    /// Whether an idle pause may be tried at `now`.
    pub fn due(retry: Option<&IdlePauseRetry>, now: SystemTime) -> bool {
        retry.is_none_or(|retry| now >= retry.retry_at)
    }

    /// The backoff after an idle pause tried at `now` did not take effect.
    /// A previous failure whose retry time is long past (the lease left the
    /// idle chain in between) no longer counts.
    pub fn after_failure(previous: Option<&IdlePauseRetry>, now: SystemTime) -> IdlePauseRetry {
        let previous_failures = previous
            .filter(|retry| now.duration_since(retry.retry_at).map_or(true, |late| late < Self::MAX))
            .map_or(0, |retry| retry.failures);
        let failures = previous_failures.saturating_add(1);
        let delay = Self::BASE.saturating_mul(1 << (failures - 1).min(16)).min(Self::MAX);
        IdlePauseRetry {
            failures,
            retry_at: now + delay,
        }
    }
}
