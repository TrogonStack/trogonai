//! Backpressure gate for `session/prompt` handling.
#![allow(dead_code)]
//!
//! **When to use**
//! - At the start of prompt handling, before publishing work to the backend.
//! - For operations where too many in-flight tasks would degrade latency or timeout behavior.
//!
//! **Why this exists**
//! - Prompt traffic can spike faster than downstream capacity; bounded concurrency keeps
//!   overload explicit (`Agent unavailable`) instead of turning into queue growth and
//!   cascading timeouts.
//! - RAII guards make release automatic on every exit path (success, error, cancellation),
//!   which prevents slot leaks under async control flow.
//! - The counter is process-local and lock-free on the hot path, so rejection decisions are
//!   cheap and deterministic under contention.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Tracks the local prompt concurrency budget.
///
/// This is intentionally narrow in scope: it enforces capacity for a single process instance.
/// System-wide fairness and ordering are owned by upstream routing / retries, not this type.
#[derive(Debug)]
pub struct PromptSlotCounter {
    max: usize,
    in_flight: AtomicUsize,
}

/// Lifetime token proving one prompt slot is currently reserved.
pub struct PromptSlotGuard<'a> {
    counter: &'a PromptSlotCounter,
}

impl Drop for PromptSlotGuard<'_> {
    fn drop(&mut self) {
        self.counter.release();
    }
}

impl PromptSlotCounter {
    fn increment_if_below_max(current: usize, max: usize) -> Option<usize> {
        (current < max).then_some(current + 1)
    }

    fn decrement_if_nonzero(current: usize) -> Option<usize> {
        current.checked_sub(1)
    }

    /// Creates a prompt slot counter.
    ///
    /// A minimum of `1` avoids misconfiguration that would permanently reject all prompts.
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            in_flight: AtomicUsize::new(0),
        }
    }

    /// Attempts to reserve capacity for one prompt request.
    ///
    /// Returns `None` when local backpressure is active and callers should reject fast
    /// (instead of enqueueing additional work).
    pub fn try_acquire(&self) -> Option<PromptSlotGuard<'_>> {
        let acquired =
            self.in_flight
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    Self::increment_if_below_max(current, self.max)
                });
        acquired.ok().map(|_| PromptSlotGuard { counter: self })
    }

    fn release(&self) {
        let _ = self.in_flight.fetch_update(
            Ordering::Release,
            Ordering::Acquire,
            Self::decrement_if_nonzero,
        );
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

    #[test]
    fn release_is_noop_when_no_slots_in_flight() {
        let counter = PromptSlotCounter::new(1);
        counter.release();
        let _g = counter.try_acquire().expect("slot should remain available");
        assert!(counter.try_acquire().is_none());
    }
}
