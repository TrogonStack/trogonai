//! Replay-protection store: dedup `jti` and PoP nonces with TTLs.
//!
//! The production-grade implementation lives in `trogon-aauth-person` backed by
//! NATS JetStream KV (TTL per key). This module provides the trait + an in-memory
//! variant used by the gateway when no shared store is configured.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

#[async_trait]
pub trait ReplayStore: Send + Sync {
    /// Returns `Ok(true)` if `key` was newly inserted and is fresh, `Ok(false)`
    /// if it was already seen. `ttl_secs` is the maximum lifetime of the record.
    async fn check_and_insert(&self, key: &str, ttl_secs: u32) -> Result<bool, ReplayError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("backend: {0}")]
    Backend(String),
}

/// Best-effort in-memory replay protection. Suitable for a single-process gateway
/// or unit tests. Multi-instance deployments should use the JetStream-backed store.
pub struct InMemoryReplayStore {
    inner: Mutex<HashMap<String, i64>>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl Default for InMemoryReplayStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryReplayStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            clock: Box::new(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                i64::try_from(secs).unwrap_or(i64::MAX)
            }),
        }
    }

    /// Inject a deterministic clock (tests only).
    #[must_use]
    pub fn with_clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn gc(&self, map: &mut HashMap<String, i64>) {
        let now = self.now();
        map.retain(|_, expires_at| *expires_at > now);
    }
}

#[async_trait]
impl ReplayStore for InMemoryReplayStore {
    async fn check_and_insert(&self, key: &str, ttl_secs: u32) -> Result<bool, ReplayError> {
        let mut map = self.inner.lock().map_err(|e| ReplayError::Backend(e.to_string()))?;
        self.gc(&mut map);
        let expires_at = self.now().saturating_add(i64::from(ttl_secs));
        if map.contains_key(key) {
            return Ok(false);
        }
        map.insert(key.to_string(), expires_at);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn first_insert_then_replay_fails() {
        let store = InMemoryReplayStore::new();
        assert!(store.check_and_insert("k1", 60).await.unwrap());
        assert!(!store.check_and_insert("k1", 60).await.unwrap());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expires_after_ttl() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let now = std::sync::Arc::new(AtomicI64::new(1000));
        let now_c = now.clone();
        let store = InMemoryReplayStore::new().with_clock(move || now_c.load(Ordering::SeqCst));
        assert!(store.check_and_insert("k", 5).await.unwrap());
        now.store(1010, Ordering::SeqCst); // past expiry
        assert!(store.check_and_insert("k", 5).await.unwrap());
    }
}
