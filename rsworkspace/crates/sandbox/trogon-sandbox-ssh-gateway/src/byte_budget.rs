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
mod tests;
