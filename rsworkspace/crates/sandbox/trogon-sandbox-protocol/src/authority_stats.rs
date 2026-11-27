use std::sync::atomic::{AtomicU64, Ordering};

/// Counters of what the guard has seen and done.
#[derive(Debug, Default)]
pub struct AuthorityStats {
    /// Leases found not to be the worker's, by the broker's answer.
    pub unowned: AtomicU64,
    /// Leases treated as unowned because checks kept failing.
    pub partitioned: AtomicU64,
    /// Leases the guard froze.
    pub frozen: AtomicU64,
    /// Leases the guard thawed.
    pub thawed: AtomicU64,
    /// Leases the guard purged.
    pub purged: AtomicU64,
    /// Checks that failed.
    pub check_failures: AtomicU64,
    /// Checks the broker could not serve because nothing answers the verb.
    pub no_responders: AtomicU64,
}

impl AuthorityStats {
    /// The current value of `counter`.
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}
