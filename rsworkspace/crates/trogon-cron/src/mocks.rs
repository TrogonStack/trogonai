//! Mock implementations for unit testing without a real NATS server.
//!
//! Enabled with the `test-support` feature:
//!
//! ```toml
//! [dev-dependencies]
//! trogon-cron = { path = "...", features = ["test-support"] }
//! ```

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use bytes::Bytes;

use crate::traits::{LeaderLock, TickPublisher};

// ── MockTickPublisher ─────────────────────────────────────────────────────────

/// Records every tick published during a test run.
#[derive(Clone, Default)]
pub struct MockTickPublisher {
    records: Arc<Mutex<Vec<PublishedTick>>>,
}

#[derive(Debug, Clone)]
pub struct PublishedTick {
    pub subject: String,
    pub payload: Vec<u8>,
}

impl MockTickPublisher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ticks(&self) -> Vec<PublishedTick> {
        self.records.lock().unwrap().clone()
    }

    pub fn tick_count(&self) -> usize {
        self.records.lock().unwrap().len()
    }

    pub fn clear(&self) {
        self.records.lock().unwrap().clear();
    }
}

impl TickPublisher for MockTickPublisher {
    type Error = std::convert::Infallible;

    async fn publish_tick(
        &self,
        subject: String,
        _headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<(), Self::Error> {
        self.records.lock().unwrap().push(PublishedTick {
            subject,
            payload: payload.to_vec(),
        });
        Ok(())
    }
}

// ── MockLeaderLock ────────────────────────────────────────────────────────────

/// Controllable leader lock for testing leader election logic.
///
/// By default, `try_acquire` always succeeds (this node is the leader).
#[derive(Clone)]
pub struct MockLeaderLock {
    allow_acquire: Arc<AtomicBool>,
    allow_renew: Arc<AtomicBool>,
    next_revision: Arc<AtomicU64>,
}

impl Default for MockLeaderLock {
    fn default() -> Self {
        Self {
            allow_acquire: Arc::new(AtomicBool::new(true)),
            allow_renew: Arc::new(AtomicBool::new(true)),
            next_revision: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl MockLeaderLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulate another node holding the lock — `try_acquire` will fail.
    pub fn deny_acquire(&self) {
        self.allow_acquire.store(false, Ordering::SeqCst);
    }

    /// Simulate losing leadership — `renew` will fail.
    pub fn deny_renew(&self) {
        self.allow_renew.store(false, Ordering::SeqCst);
    }

    pub fn allow_acquire(&self) {
        self.allow_acquire.store(true, Ordering::SeqCst);
    }

    pub fn allow_renew(&self) {
        self.allow_renew.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub struct MockLockError(pub &'static str);

impl std::fmt::Display for MockLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MockLockError {}

impl LeaderLock for MockLeaderLock {
    type Error = MockLockError;

    async fn try_acquire(&self, _value: Bytes) -> Result<u64, MockLockError> {
        if self.allow_acquire.load(Ordering::SeqCst) {
            Ok(self.next_revision.fetch_add(1, Ordering::SeqCst))
        } else {
            Err(MockLockError("lock held by another node"))
        }
    }

    async fn renew(&self, _value: Bytes, revision: u64) -> Result<u64, MockLockError> {
        if self.allow_renew.load(Ordering::SeqCst) {
            Ok(revision + 1)
        } else {
            Err(MockLockError("lost leadership"))
        }
    }

    async fn release(&self) -> Result<(), MockLockError> {
        Ok(())
    }
}
