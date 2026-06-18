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
mod tests {
    use super::*;

    #[test]
    fn guard_increments_on_creation_decrements_on_drop() {
        let counter = Rc::new(Cell::new(0usize));
        assert_eq!(counter.get(), 0);

        let guard = InFlightSlotGuard::new(counter.clone());
        assert_eq!(counter.get(), 1);

        drop(guard);
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn multiple_guards_track_independently() {
        let counter = Rc::new(Cell::new(0usize));
        let g1 = InFlightSlotGuard::new(counter.clone());
        let g2 = InFlightSlotGuard::new(counter.clone());
        assert_eq!(counter.get(), 2);

        drop(g1);
        assert_eq!(counter.get(), 1);

        drop(g2);
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn saturating_sub_avoids_underflow() {
        let counter = Rc::new(Cell::new(0usize));
        let guard = InFlightSlotGuard::new(counter.clone());
        counter.set(0);
        drop(guard);
        assert_eq!(counter.get(), 0);
    }
}
