use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{CachedRelease, ReleaseCacheKey, ReleaseCacheMode, ReleaseInvalidation, ReleasedValue};

/// The releases one client holds, bounded in count and in time.
///
/// A copy expires at its hold, pulled ahead by its key's jitter, so it never
/// outlives the value's `valid_until` nor [`crate::RELEASE_HOLD_BOUND`] after
/// it arrived (ADR#0049).
///
/// A release asked for before an invalidation, or before the reader lost its
/// place, may be the value that invalidation dropped, so it is served but not
/// held: every change bumps the epoch, and a release whose ticket names an
/// older epoch is not kept.
#[derive(Debug)]
pub(crate) struct ReleaseCache {
    entries: BTreeMap<ReleaseCacheKey, CachedRelease>,
    capacity: usize,
    epoch: u64,
    mode: ReleaseCacheMode,
}

impl ReleaseCache {
    pub(crate) const fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity,
            epoch: 0,
            mode: ReleaseCacheMode::Caching,
        }
    }

    /// The copy held under `key`, if it still serves at `now`.
    pub(crate) fn get(&mut self, key: &ReleaseCacheKey, now: SystemTime) -> Option<ReleasedValue> {
        let held = self.entries.get(key)?;
        if held.is_live_at(now) {
            return Some(held.value().clone());
        }
        self.entries.remove(key);
        None
    }

    /// Names the epoch a release is asked for in, to hand back to
    /// [`ReleaseCache::put`] once it arrives.
    pub(crate) const fn ticket(&self) -> u64 {
        self.epoch
    }

    /// Holds `value` under `key` as it arrived at `now`, unless anything
    /// changed since `ticket` was taken, its hold is over, or the cache is
    /// full of copies that still serve.
    pub(crate) fn put(&mut self, ticket: u64, key: ReleaseCacheKey, value: ReleasedValue, now: SystemTime) {
        if self.mode == ReleaseCacheMode::Uncached || ticket != self.epoch {
            return;
        }
        let Ok(ttl) = value.hold_until().duration_since(now) else {
            return;
        };
        if ttl.is_zero() {
            return;
        }
        let expires_at = value.hold_until() - key.jitter_within(ttl);
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            self.entries.retain(|_, held| held.is_live_at(now));
            if self.entries.len() >= self.capacity {
                return;
            }
        }
        self.entries.insert(key, CachedRelease::new(value, expires_at));
    }

    /// Drops every copy `notice` covers.
    pub(crate) fn invalidate(&mut self, notice: &ReleaseInvalidation) {
        self.epoch += 1;
        self.entries.retain(|key, _| !notice.covers(key));
    }

    /// Drops every copy and holds none until [`ReleaseCache::resume`].
    pub(crate) fn suspend(&mut self) {
        self.epoch += 1;
        self.mode = ReleaseCacheMode::Uncached;
        self.entries.clear();
    }

    /// Holds releases again.
    pub(crate) fn resume(&mut self) {
        self.epoch += 1;
        self.mode = ReleaseCacheMode::Caching;
    }

    pub(crate) const fn mode(&self) -> ReleaseCacheMode {
        self.mode
    }
}

#[cfg(test)]
mod tests;
