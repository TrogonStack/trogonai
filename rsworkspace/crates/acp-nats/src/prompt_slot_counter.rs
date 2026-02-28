//! Backpressure counter for concurrent prompt requests.
//!
//! Limits in-flight prompt requests to a configured maximum. Acquire a slot before
//! publishing a prompt; the returned [`PromptSlotGuard`] releases it automatically on drop.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Thread-safe counter that limits concurrent prompt requests.
///
/// [`try_acquire`](Self::try_acquire) returns an RAII guard that releases the slot on drop,
/// making it impossible to forget to release or double-release.
#[derive(Debug)]
pub struct PromptSlotCounter {
    max: usize,
    in_flight: AtomicUsize,
}

/// RAII guard returned by [`PromptSlotCounter::try_acquire`].
/// Releases the acquired slot when dropped.
pub struct PromptSlotGuard<'a> {
    counter: &'a PromptSlotCounter,
}

impl Drop for PromptSlotGuard<'_> {
    fn drop(&mut self) {
        self.counter.release();
    }
}

impl PromptSlotCounter {
    /// Creates a counter with the given maximum concurrent slots.
    /// Enforces a minimum of 1.
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            in_flight: AtomicUsize::new(0),
        }
    }

    /// Attempts to acquire a slot. Returns a guard that releases the slot on drop,
    /// or `None` if all slots are in use.
    pub fn try_acquire(&self) -> Option<PromptSlotGuard<'_>> {
        loop {
            let current = self.in_flight.load(Ordering::Acquire);
            if current >= self.max {
                return None;
            }
            if self
                .in_flight
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(PromptSlotGuard { counter: self });
            }
        }
    }

    fn release(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::Release, Ordering::Acquire, |v| {
                if v > 0 { Some(v - 1) } else { None }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_acquire_succeeds_until_max() {
        let counter = PromptSlotCounter::new(2);
        let _g1 = counter.try_acquire().expect("slot 1");
        let _g2 = counter.try_acquire().expect("slot 2");
        assert!(counter.try_acquire().is_none());
    }

    #[test]
    fn release_frees_slot() {
        let counter = PromptSlotCounter::new(1);
        let guard = counter.try_acquire().expect("slot");
        assert!(counter.try_acquire().is_none());
        drop(guard);
        assert!(counter.try_acquire().is_some());
    }

    #[test]
    fn new_enforces_minimum_one() {
        let counter = PromptSlotCounter::new(0);
        let _g = counter.try_acquire().expect("slot");
        assert!(counter.try_acquire().is_none());
    }

    #[test]
    fn guard_drop_releases_exactly_one_slot() {
        let counter = PromptSlotCounter::new(2);
        let g1 = counter.try_acquire().expect("slot 1");
        let _g2 = counter.try_acquire().expect("slot 2");
        assert!(counter.try_acquire().is_none());
        drop(g1);
        let _g3 = counter.try_acquire().expect("slot freed by g1 drop");
        assert!(counter.try_acquire().is_none());
    }

    #[test]
    fn multiple_guards_release_independently() {
        let counter = PromptSlotCounter::new(3);
        let g1 = counter.try_acquire().expect("slot 1");
        let g2 = counter.try_acquire().expect("slot 2");
        let g3 = counter.try_acquire().expect("slot 3");
        assert!(counter.try_acquire().is_none());
        drop(g2);
        let _g4 = counter.try_acquire().expect("slot freed by g2");
        assert!(counter.try_acquire().is_none());
        drop(g1);
        drop(g3);
        let _g5 = counter.try_acquire().expect("slot freed by g1");
        let _g6 = counter.try_acquire().expect("slot freed by g3");
        assert!(counter.try_acquire().is_none());
    }
}
