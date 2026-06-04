//! TTL-caching wrapper around any `JwksResolver`.
//!
//! Production deployments resolve JWKS over HTTP through OIDC discovery, which
//! makes per-request fetches both slow and a denial-of-service risk against the
//! upstream identity provider. This module caches resolved key sets for a
//! positive TTL and also caches the most recent failure for a (shorter)
//! negative TTL so a flapping IdP cannot turn into a hot loop of outbound
//! requests.

use std::collections::HashMap;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use tokio::sync::RwLock;

use crate::jwks::{JwksError, JwksResolver};
use crate::time_source::TimeSource;

/// Default positive TTL for cached JWK sets. Matches the documented OpenAI
/// Workload Identity Federation discovery cache.
pub const DEFAULT_TTL_SECS: i64 = 600;
/// Default negative TTL for cached failures. Short enough that a transient
/// outage at the upstream IdP self-heals quickly without becoming a tight loop.
pub const DEFAULT_NEGATIVE_TTL_SECS: i64 = 30;

enum CachedEntry {
    Hit { keys: JwkSet, expires_at: i64 },
    Miss { error: String, expires_at: i64 },
}

impl CachedEntry {
    fn is_fresh(&self, now: i64) -> bool {
        match self {
            CachedEntry::Hit { expires_at, .. } | CachedEntry::Miss { expires_at, .. } => now < *expires_at,
        }
    }
}

/// TTL-caching wrapper around any `JwksResolver`.
pub struct CachedJwksResolver<R: JwksResolver, T: TimeSource> {
    inner: R,
    time: T,
    ttl_secs: i64,
    negative_ttl_secs: i64,
    entries: RwLock<HashMap<String, CachedEntry>>,
}

impl<R: JwksResolver, T: TimeSource> CachedJwksResolver<R, T> {
    #[must_use]
    pub fn new(inner: R, time: T) -> Self {
        Self {
            inner,
            time,
            ttl_secs: DEFAULT_TTL_SECS,
            negative_ttl_secs: DEFAULT_NEGATIVE_TTL_SECS,
            entries: RwLock::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn with_ttl_secs(mut self, ttl_secs: i64) -> Self {
        self.ttl_secs = ttl_secs;
        self
    }

    #[must_use]
    pub fn with_negative_ttl_secs(mut self, ttl_secs: i64) -> Self {
        self.negative_ttl_secs = ttl_secs;
        self
    }

    /// Drop every cached entry. Operators use this after rotating provider
    /// trust material so the next verification re-fetches.
    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
    }

    /// Drop the cached entry for a single issuer.
    pub async fn invalidate(&self, iss: &str) {
        self.entries.write().await.remove(iss);
    }
}

#[async_trait]
impl<R: JwksResolver, T: TimeSource> JwksResolver for CachedJwksResolver<R, T> {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
        let now = self.time.now();
        if let Some(entry) = self.entries.read().await.get(iss)
            && entry.is_fresh(now)
        {
            return match entry {
                CachedEntry::Hit { keys, .. } => Ok(keys.clone()),
                CachedEntry::Miss { error, .. } => Err(JwksError::Transport(error.clone())),
            };
        }

        let outcome = self.inner.resolve(iss).await;
        let mut guard = self.entries.write().await;
        match &outcome {
            Ok(keys) => {
                guard.insert(
                    iss.to_string(),
                    CachedEntry::Hit {
                        keys: keys.clone(),
                        expires_at: now.saturating_add(self.ttl_secs),
                    },
                );
            }
            Err(err) => {
                guard.insert(
                    iss.to_string(),
                    CachedEntry::Miss {
                        error: err.to_string(),
                        expires_at: now.saturating_add(self.negative_ttl_secs),
                    },
                );
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    use super::*;
    use crate::jwks::StaticJwks;

    #[derive(Clone, Default)]
    struct MockTime {
        now: Arc<AtomicI64>,
    }

    impl MockTime {
        fn at(secs: i64) -> Self {
            Self {
                now: Arc::new(AtomicI64::new(secs)),
            }
        }
        fn advance(&self, secs: i64) {
            self.now.fetch_add(secs, Ordering::SeqCst);
        }
    }

    impl TimeSource for MockTime {
        fn now(&self) -> i64 {
            self.now.load(Ordering::SeqCst)
        }
    }

    struct CountingResolver {
        inner: StaticJwks,
        calls: AtomicUsize,
        fail_with: Mutex<Option<JwksError>>,
    }

    impl CountingResolver {
        fn new(inner: StaticJwks) -> Self {
            Self {
                inner,
                calls: AtomicUsize::new(0),
                fail_with: Mutex::new(None),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn fail_next(&self, err: JwksError) {
            *self.fail_with.lock().unwrap() = Some(err);
        }
    }

    #[async_trait]
    impl JwksResolver for CountingResolver {
        async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(err) = self.fail_with.lock().unwrap().take() {
                return Err(err);
            }
            self.inner.resolve(iss).await
        }
    }

    fn empty_set() -> JwkSet {
        JwkSet { keys: vec![] }
    }

    #[tokio::test]
    async fn hits_inner_once_within_ttl() {
        let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
        let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_ttl_secs(60);

        cache.resolve("iss.example").await.expect("first hit");
        cache.resolve("iss.example").await.expect("cached hit");
        cache.resolve("iss.example").await.expect("cached hit");

        assert_eq!(cache.inner.calls(), 1);
    }

    #[tokio::test]
    async fn re_fetches_after_ttl_expires() {
        let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
        let time = MockTime::at(0);
        let cache = CachedJwksResolver::new(inner, time.clone()).with_ttl_secs(60);

        cache.resolve("iss.example").await.expect("first hit");
        time.advance(61);
        cache.resolve("iss.example").await.expect("re-fetched");

        assert_eq!(cache.inner.calls(), 2);
    }

    #[tokio::test]
    async fn negative_cache_dampens_failure_storms() {
        let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
        inner.fail_next(JwksError::Transport("boom".into()));
        let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_negative_ttl_secs(30);

        let first = cache.resolve("iss.example").await;
        assert!(matches!(first, Err(JwksError::Transport(_))));
        let second = cache.resolve("iss.example").await;
        assert!(matches!(second, Err(JwksError::Transport(_))));

        assert_eq!(cache.inner.calls(), 1, "negative cache held the second call");
    }

    #[tokio::test]
    async fn invalidate_clears_entry() {
        let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
        let cache = CachedJwksResolver::new(inner, MockTime::at(0));

        cache.resolve("iss.example").await.expect("first hit");
        cache.invalidate("iss.example").await;
        cache.resolve("iss.example").await.expect("re-fetched");

        assert_eq!(cache.inner.calls(), 2);
    }
}
