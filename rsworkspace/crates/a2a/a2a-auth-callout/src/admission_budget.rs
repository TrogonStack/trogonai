use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::admission_limit::AdmissionLimit;

#[derive(Clone)]
pub struct AdmissionBudget {
    permits: Arc<Semaphore>,
}

impl AdmissionBudget {
    pub fn new(limit: AdmissionLimit) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limit.get())),
        }
    }

    pub(crate) fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }
}

impl Default for AdmissionBudget {
    fn default() -> Self {
        Self::new(AdmissionLimit::default())
    }
}
