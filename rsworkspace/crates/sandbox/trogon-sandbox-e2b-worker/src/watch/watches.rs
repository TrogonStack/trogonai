use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_nats::jetstream;

use super::constants::{DEFAULT_POLL_INTERVAL, MIN_POLL_INTERVAL, RELEASE_TIMEOUT, WATCH_IDLE_EXPIRY};
use super::watch_entry::WatchEntry;
use crate::envd::Envd;

pub(super) type LeaseWatches = HashMap<String, Arc<WatchEntry>>;

/// Every lease's watches on this worker.
pub struct Watches {
    pub(super) envd: Envd,
    leases: Mutex<HashMap<String, LeaseWatches>>,
    pub(super) ids: AtomicU64,
    pub(super) poll_interval: Duration,
    pub(super) idle_expiry: Duration,
    pub(super) release_timeout: Duration,
    pub(super) evidence: Option<jetstream::Context>,
}

impl Watches {
    pub fn new(envd: Envd) -> Self {
        Self {
            envd,
            leases: Mutex::default(),
            ids: AtomicU64::new(0),
            poll_interval: DEFAULT_POLL_INTERVAL,
            idle_expiry: WATCH_IDLE_EXPIRY,
            release_timeout: RELEASE_TIMEOUT,
            evidence: None,
        }
    }

    /// Publishes a watch's overflow on the lease's evidence timeline.
    pub fn with_evidence(mut self, js: jetstream::Context) -> Self {
        self.evidence = Some(js);
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval.max(MIN_POLL_INTERVAL);
        self
    }

    /// How long a watch may go unpolled before it is removed.
    pub fn with_idle_expiry(mut self, expiry: Duration) -> Self {
        self.idle_expiry = expiry;
        self
    }

    /// How long releasing a lease waits for envd to drop its watchers.
    pub fn with_release_timeout(mut self, timeout: Duration) -> Self {
        self.release_timeout = timeout;
        self
    }

    pub(super) fn leases(&self) -> MutexGuard<'_, HashMap<String, LeaseWatches>> {
        self.leases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Watches live on `lease`.
    pub fn count(&self, lease: &str) -> usize {
        self.leases().get(lease).map_or(0, HashMap::len)
    }

    /// Drops a lease's watches locally, without calling envd.
    pub fn forget_lease(&self, lease: &str) {
        for watch in self.leases().remove(lease).into_iter().flat_map(HashMap::into_values) {
            watch.gone.cancel();
        }
    }
}
