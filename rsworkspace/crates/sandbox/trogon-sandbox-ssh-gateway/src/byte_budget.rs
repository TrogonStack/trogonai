use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Counts received-but-unwritten input bytes against a fixed budget.
#[derive(Clone)]
pub(crate) struct ByteBudget {
    permits: Arc<Semaphore>,
}

impl ByteBudget {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max.min(Semaphore::MAX_PERMITS))),
        }
    }

    /// Takes `bytes` of the budget, or `None` when that would exceed it.
    pub(crate) fn take(&self, bytes: usize) -> Option<OwnedSemaphorePermit> {
        let bytes = u32::try_from(bytes).ok()?;
        Arc::clone(&self.permits).try_acquire_many_owned(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_take_past_the_cap_is_refused() {
        let budget = ByteBudget::new(10);
        let held = budget.take(6);
        assert!(held.is_some());
        assert!(budget.take(5).is_none());
        assert!(budget.take(4).is_some());
    }

    #[test]
    fn releasing_a_take_refills_the_budget() {
        let budget = ByteBudget::new(10);
        let held = budget.take(10);
        assert!(held.is_some());
        assert!(budget.take(1).is_none());
        drop(held);
        assert!(budget.take(10).is_some());
    }

    #[test]
    fn a_take_larger_than_any_budget_is_refused() {
        let budget = ByteBudget::new(usize::MAX);
        assert!(budget.take(usize::MAX).is_none());
    }
}
