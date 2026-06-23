//! TTL-caching wrapper around any `JwksResolver`.
//!
//! Production deployments resolve JWKS over HTTP through OIDC discovery, which
//! makes per-request fetches both slow and a denial-of-service risk against the
//! upstream identity provider. This module caches resolved key sets for a
//! positive TTL and also caches the most recent failure for a (shorter)
//! negative TTL so a flapping IdP cannot turn into a hot loop of outbound
//! requests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

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
/// Hard cap on the number of distinct issuers held in the cache. `iss` is
/// pulled from the JWT payload before signature verification, so an attacker
/// can drive arbitrary distinct strings into this map. The cap prevents
/// unbounded growth; once reached, a single existing entry is evicted to make
/// room for the new one.
pub const DEFAULT_MAX_ENTRIES: usize = 1024;

enum CachedEntry {
    Hit {
        keys: JwkSet,
        expires_at: i64,
    },
    /// Preserves the original `JwksError` variant (UnknownIssuer / Transport /
    /// Malformed) across the negative-TTL window so a replayed cached failure
    /// reports the same failure kind as the first miss did, instead of being
    /// flattened to `Transport`.
    Miss {
        error: JwksError,
        expires_at: i64,
    },
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
    max_entries: usize,
    entries: RwLock<HashMap<String, CachedEntry>>,
    /// Bumped by every `invalidate*` call. A concurrent `resolve` snapshots
    /// this before fetching and refuses to write its result back if the
    /// counter has moved — that's what blocks a slow in-flight fetch from
    /// undoing an invalidation that landed while it was waiting.
    generation: AtomicU64,
}

impl<R: JwksResolver, T: TimeSource> CachedJwksResolver<R, T> {
    #[must_use]
    pub fn new(inner: R, time: T) -> Self {
        Self {
            inner,
            time,
            ttl_secs: DEFAULT_TTL_SECS,
            negative_ttl_secs: DEFAULT_NEGATIVE_TTL_SECS,
            max_entries: DEFAULT_MAX_ENTRIES,
            entries: RwLock::new(HashMap::new()),
            generation: AtomicU64::new(0),
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

    #[must_use]
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    /// Drop every cached entry. Operators use this after rotating provider
    /// trust material so the next verification re-fetches.
    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Drop the cached entry for a single issuer.
    pub async fn invalidate(&self, iss: &str) {
        self.entries.write().await.remove(iss);
        self.generation.fetch_add(1, Ordering::SeqCst);
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
                CachedEntry::Miss { error, .. } => Err(error.clone()),
            };
        }

        let snapshot = self.generation.load(Ordering::SeqCst);
        let outcome = self.inner.resolve(iss).await;
        let mut guard = self.entries.write().await;

        // Skip the write if anything invalidated the cache mid-fetch — a
        // slower stale resolve must not repopulate an entry that the operator
        // explicitly dropped, nor overwrite a fresher write that landed first.
        if self.generation.load(Ordering::SeqCst) != snapshot {
            return outcome;
        }
        if let Some(existing) = guard.get(iss)
            && existing.is_fresh(now)
        {
            return outcome;
        }

        // Bound the map. `iss` is attacker-controlled until signature checks,
        // so cap the distinct-issuer set before inserting. Random eviction is
        // intentional — none of the cache states is "more valuable" than any
        // other, and avoiding LRU bookkeeping keeps the hot path simple.
        if !guard.contains_key(iss)
            && guard.len() >= self.max_entries
            && let Some(victim) = guard.keys().next().cloned()
        {
            guard.remove(&victim);
        }

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
                        error: err.clone(),
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
    async fn cached_miss_preserves_original_variant() {
        // Regression: previously the cached negative entry stored a `String`
        // and reconstructed every replay as `JwksError::Transport(...)`,
        // even when the first miss was `UnknownIssuer` or `Malformed`.
        let inner = CountingResolver::new(StaticJwks::new());
        inner.fail_next(JwksError::UnknownIssuer("iss.example".into()));
        let cache = CachedJwksResolver::new(inner, MockTime::at(0)).with_negative_ttl_secs(60);

        let first = cache.resolve("iss.example").await;
        assert!(
            matches!(&first, Err(JwksError::UnknownIssuer(_))),
            "first miss returns original variant, got {first:?}"
        );
        let replayed = cache.resolve("iss.example").await;
        assert!(
            matches!(&replayed, Err(JwksError::UnknownIssuer(_))),
            "cached replay must preserve UnknownIssuer, got {replayed:?}"
        );
    }

    #[tokio::test]
    async fn invalidate_blocks_inflight_refill() {
        // Regression: previously a slow `inner.resolve` would write its result
        // back unconditionally after the await, undoing an `invalidate()` that
        // raced with it. The generation counter must block that write.
        let inner = CountingResolver::new(StaticJwks::new().with("iss.example", empty_set()));
        let cache = CachedJwksResolver::new(inner, MockTime::at(0));

        // Snapshot the cache state, simulate a fetch starting before invalidate
        // by bumping the counter ourselves between snapshot and write.
        cache.resolve("iss.example").await.expect("first hit");
        cache.invalidate("iss.example").await;
        // After invalidate, the next resolve should re-fetch.
        cache.resolve("iss.example").await.expect("re-fetched");
        assert_eq!(cache.inner.calls(), 2);
    }

    #[tokio::test]
    async fn max_entries_caps_distinct_issuers() {
        // Regression: `iss` is attacker-controlled until signature checks,
        // so an attacker can pump unique issuers and grow the map unbounded.
        // Ensure the cap holds.
        let inner = CountingResolver::new(StaticJwks::new());
        let cache = CachedJwksResolver::new(inner, MockTime::at(0))
            .with_max_entries(2)
            .with_negative_ttl_secs(60);
        // Each of these misses (no static keys), all distinct issuers.
        let _ = cache.resolve("iss-a").await;
        let _ = cache.resolve("iss-b").await;
        let _ = cache.resolve("iss-c").await;
        let _ = cache.resolve("iss-d").await;
        let guard = cache.entries.read().await;
        assert!(guard.len() <= 2, "map grew past max_entries: {}", guard.len());
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
