use std::time::Duration;

/// The capture claim outlives four API round trips plus this slack.
pub(super) const CAPTURE_SLACK: Duration = Duration::from_secs(60);

/// How many times custody is re-read and re-written before giving up on claiming it.
pub(super) const CLAIM_ATTEMPTS: usize = 8;

/// The cleanup claim outlives two API round trips plus this slack.
pub(super) const CLEANUP_SLACK: Duration = Duration::from_secs(5);

/// Largest retry backoff exponent: 2^8 seconds.
pub(super) const MAX_BACKOFF_EXPONENT: u32 = 8;

/// Cleanups one recovery pass may attempt, so a large backlog cannot starve the worker.
pub(super) const MAX_CLEANUPS_PER_PASS: usize = 16;
