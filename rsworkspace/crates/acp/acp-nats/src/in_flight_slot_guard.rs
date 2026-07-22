//! RAII guard for client proxy in-flight task backpressure.
//!
//! Tracks concurrent client request handlers; increments on construction,
//! decrements on drop so slots are released on every exit path.

use std::cell::Cell;
use std::rc::Rc;

/// Lifetime token proving one client task slot is currently reserved.
pub(crate) struct InFlightSlotGuard(Rc<Cell<usize>>);

impl InFlightSlotGuard {
    pub(crate) fn new(counter: Rc<Cell<usize>>) -> Self {
        counter.set(counter.get().saturating_add(1));
        Self(counter)
    }
}

impl Drop for InFlightSlotGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests;
