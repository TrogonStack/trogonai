use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use trogon_std::time::GetElapsed;

use super::constants::{FULL_CACHE_WARNING_INTERVAL, RECENT_INVALIDATIONS, RELEASE_HOLD_BOUND};
use super::{CachedRelease, ReleaseCacheKey, ReleaseCacheMode, ReleaseHold, ReleaseInvalidation, ReleasedValue};

/// The releases one client holds, bounded in count and in time.
///
/// A copy expires at its hold, pulled ahead by its key's jitter, so it never
/// outlives the value's `valid_until` nor [`crate::RELEASE_HOLD_BOUND`] after
/// it arrived (ADR#0049). It also expires that long after it was held on
/// `clock`, a monotonic clock, and never more than [`RELEASE_HOLD_BOUND`]
/// after, so a wall clock stepped back cannot keep a copy serving past its
/// hold.
///
/// A release asked for before an invalidation that covers its key, or before
/// the reader lost or regained its place, may be the value that change
/// revoked, so it is neither held nor served. Every change bumps the epoch
/// and is remembered, up to [`RECENT_INVALIDATIONS`] of them; a ticket names
/// the epoch its release was asked for in, and it stays current only while
/// every change since covers other keys. A ticket older than every change
/// remembered is not current, since what it missed is no longer known.
///
/// A release turned away because every slot holds a copy that still serves
/// is counted toward the next warning that the cache is full, due at the
/// first one and then at most once every [`FULL_CACHE_WARNING_INTERVAL`] on
/// `clock`.
pub(crate) struct ReleaseCache<C: GetElapsed> {
    entries: BTreeMap<ReleaseCacheKey, CachedRelease<C::Instant>>,
    capacity: usize,
    clock: C,
    epoch: u64,
    recent: VecDeque<Option<ReleaseInvalidation>>,
    mode: ReleaseCacheMode,
    turned_away: u64,
    warned_at: Option<C::Instant>,
}

impl<C: GetElapsed> ReleaseCache<C> {
    pub(crate) const fn with_capacity(capacity: usize, clock: C) -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity,
            clock,
            epoch: 0,
            recent: VecDeque::new(),
            mode: ReleaseCacheMode::Caching,
            turned_away: 0,
            warned_at: None,
        }
    }

    /// The most releases it holds at once.
    pub(crate) const fn capacity(&self) -> usize {
        self.capacity
    }

    /// The copy held under `key`, if it still serves at `now`.
    pub(crate) fn get(&mut self, key: &ReleaseCacheKey, now: SystemTime) -> Option<ReleasedValue> {
        let held = self.entries.get(key)?;
        if held.is_live_at(now, &self.clock) {
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

    /// Whether a release for `key` asked for at `ticket` may still be served:
    /// nothing since covers `key`, the reader neither lost nor regained its
    /// place, and every change since is still remembered.
    pub(crate) fn is_current(&self, ticket: u64, key: &ReleaseCacheKey) -> bool {
        let since = usize::try_from(self.epoch - ticket).unwrap_or(usize::MAX);
        since <= self.recent.len()
            && self
                .recent
                .iter()
                .rev()
                .take(since)
                .all(|change| change.as_ref().is_some_and(|notice| !notice.covers(key)))
    }

    /// Holds `value` under `key` as it arrived at `now`, unless it is no
    /// longer current for `ticket`, its hold is over, or the cache is full of
    /// copies that still serve.
    pub(crate) fn put(
        &mut self,
        ticket: u64,
        key: ReleaseCacheKey,
        value: ReleasedValue,
        now: SystemTime,
    ) -> ReleaseHold {
        if self.mode == ReleaseCacheMode::Uncached || !self.is_current(ticket, &key) {
            return ReleaseHold::Declined;
        }
        let Ok(ttl) = value.hold_until().duration_since(now) else {
            return ReleaseHold::Declined;
        };
        if ttl.is_zero() {
            return ReleaseHold::Declined;
        }
        let jitter = key.jitter_within(ttl);
        let expires_at = value.hold_until() - jitter;
        let lifetime = (ttl - jitter).min(RELEASE_HOLD_BOUND);
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            self.entries.retain(|_, held| held.is_live_at(now, &self.clock));
            if self.entries.len() >= self.capacity {
                self.turned_away += 1;
                return ReleaseHold::Full;
            }
        }
        let held = CachedRelease::new(value, expires_at, self.clock.now(), lifetime);
        self.entries.insert(key, held);
        ReleaseHold::Held
    }

    /// How many releases were turned away for room since the last warning,
    /// when a warning that the cache is full is due.
    pub(crate) fn capacity_warning(&mut self) -> Option<u64> {
        if self.turned_away == 0
            || self
                .warned_at
                .is_some_and(|warned_at| self.clock.elapsed(warned_at) < FULL_CACHE_WARNING_INTERVAL)
        {
            return None;
        }
        self.warned_at = Some(self.clock.now());
        Some(std::mem::take(&mut self.turned_away))
    }

    /// Drops every copy `notice` covers.
    pub(crate) fn invalidate(&mut self, notice: &ReleaseInvalidation) {
        self.entries.retain(|key, _| !notice.covers(key));
        self.change(Some(notice.clone()));
    }

    /// Drops every copy and holds none until [`ReleaseCache::resume`].
    pub(crate) fn suspend(&mut self) {
        self.change(None);
        self.mode = ReleaseCacheMode::Uncached;
        self.entries.clear();
    }

    /// Holds releases again.
    pub(crate) fn resume(&mut self) {
        self.change(None);
        self.mode = ReleaseCacheMode::Caching;
    }

    /// Bumps the epoch and remembers what changed: the notice, or `None` when
    /// the reader lost or regained its place, which covers every key.
    fn change(&mut self, notice: Option<ReleaseInvalidation>) {
        self.epoch += 1;
        self.recent.push_back(notice);
        if self.recent.len() > RECENT_INVALIDATIONS {
            self.recent.pop_front();
        }
    }

    pub(crate) const fn mode(&self) -> ReleaseCacheMode {
        self.mode
    }
}

#[cfg(test)]
mod tests;
