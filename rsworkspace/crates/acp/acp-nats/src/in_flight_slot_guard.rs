//! RAII guard for client proxy in-flight task backpressure.
//!
//! Tracks concurrent client request handlers; increments on construction,
//! decrements on drop so slots are released on every exit path.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Lifetime token proving one client task slot is currently reserved.
pub(crate) struct InFlightSlotGuard(Arc<AtomicUsize>);

impl InFlightSlotGuard {
    pub(crate) fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Drop for InFlightSlotGuard {
    fn drop(&mut self) {
        // `fetch_update` rather than `fetch_sub`: a counter already at zero must
        // saturate, not wrap to `usize::MAX`. Preserves the `saturating_sub`
        // behavior the single-threaded `Cell` version had.
        let _ = self.0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.saturating_sub(1))
        });
    }
}

#[cfg(test)]
mod tests;
