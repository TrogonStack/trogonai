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
        let mut guard = self.entries.write().await;
        // Bump the generation BEFORE releasing the write guard. Otherwise a
        // concurrent resolve waiting on the write lock could acquire it after
        // `clear()`/`remove()` ran but before `fetch_add` landed, observe the
        // pre-bump generation, and insert a pre-rotation outcome back into
        // the cache.
        self.generation.fetch_add(1, Ordering::SeqCst);
        guard.clear();
    }

    /// Drop the cached entry for a single issuer.
    pub async fn invalidate(&self, iss: &str) {
        let mut guard = self.entries.write().await;
        self.generation.fetch_add(1, Ordering::SeqCst);
        guard.remove(iss);
    }
}

/// Maximum number of resolve retries when a concurrent `invalidate*` lands
/// while a fetch is in flight. Bounded so an adversarial invalidate loop
/// cannot pin a caller in an unbounded retry cycle.
const MAX_INVALIDATE_RETRIES: usize = 3;

#[async_trait]
impl<R: JwksResolver, T: TimeSource> JwksResolver for CachedJwksResolver<R, T> {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
        // Sample wall time fresh at every step. The fast read-lock check, the
        // slow-path freshness check after `inner.resolve`, and the `expires_at`
        // we write all need to use the wall time AT THAT POINT — otherwise a
        // slow JWKS fetch (or a long lock wait) lets us serve expired entries
        // and lets new entries live longer than the configured TTL.
        if let Some(entry) = self.entries.read().await.get(iss)
            && entry.is_fresh(self.time.now())
        {
            return match entry {
                CachedEntry::Hit { keys, .. } => Ok(keys.clone()),
                CachedEntry::Miss { error, .. } => Err(error.clone()),
            };
        }

        // Re-fetch when the generation moves mid-flight. A pre-rotation fetch
        // that started before invalidate() must NOT supply stale keys back to
        // the verifier — that defeats operator-driven key rotation. We re-run
        // up to MAX_INVALIDATE_RETRIES times and only then yield whatever the
        // last fetch returned, to bound work under an adversarial invalidate
        // loop.
        let mut last_outcome: Option<Result<JwkSet, JwksError>> = None;
        for _ in 0..=MAX_INVALIDATE_RETRIES {
            let snapshot = self.generation.load(Ordering::SeqCst);
            let outcome = self.inner.resolve(iss).await;
            let mut guard = self.entries.write().await;
            // Re-sample after the inner.resolve await — that's the only clock
            // value that the TTL of the entry we're about to write should be
            // anchored to.
            let now = self.time.now();

            if self.generation.load(Ordering::SeqCst) != snapshot {
                // Operator invalidated between snapshot and now. The outcome
                // we just got could be pre-rotation — drop it and retry.
                last_outcome = Some(outcome);
                drop(guard);
                continue;
            }
            if let Some(existing) = guard.get(iss)
                && existing.is_fresh(now)
            {
                // A concurrent resolver populated a fresh entry while we
                // were out. Return that entry — NOT our own outcome — so two
                // concurrent callers cannot disagree about the cache state
                // (e.g. a transient error here while the peer landed a Hit
                // would otherwise surface as a phantom failure).
                return match existing {
                    CachedEntry::Hit { keys, .. } => Ok(keys.clone()),
                    CachedEntry::Miss { error, .. } => Err(error.clone()),
                };
            }

            // Bound the map. `iss` is attacker-controlled until signature
            // checks, so cap the distinct-issuer set before inserting. Random
            // eviction is intentional — none of the cache states is "more
            // valuable" than any other, and avoiding LRU bookkeeping keeps
            // the hot path simple.
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
            return outcome;
        }
        // Adversarial invalidate-loop fallback: return the last fetch outcome
        // unwritten. The next caller pays one more upstream fetch instead of
        // observing stale keys from this caller's cache write. The loop
        // always runs at least once, so last_outcome is always Some.
        match last_outcome {
            Some(outcome) => outcome,
            // Defensively re-issue rather than panic if the loop somehow
            // never populated last_outcome (only reachable if
            // MAX_INVALIDATE_RETRIES is reduced to 0).
            None => self.inner.resolve(iss).await,
        }
    }
}

#[cfg(test)]
mod tests;
