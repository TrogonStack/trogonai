use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::audit_capacity::AuditCapacity;
use crate::audit_error::AuditError;
use crate::connection_limit::ConnectionLimit;

pub(crate) struct AuditBudget {
    limit: ConnectionLimit,
    slots: Arc<Semaphore>,
}

impl AuditBudget {
    pub(crate) fn new(limit: ConnectionLimit) -> Self {
        Self {
            limit,
            slots: Arc::new(Semaphore::new(limit.get())),
        }
    }

    pub(crate) fn try_acquire(&self) -> Result<OwnedSemaphorePermit, AuditError> {
        Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| AuditError::Overloaded)
    }

    pub(crate) fn capacity(&self) -> AuditCapacity {
        AuditCapacity::new(
            self.limit,
            self.limit.get() - self.slots.available_permits(),
            !self.slots.is_closed(),
        )
    }

    pub(crate) fn close(&self) {
        self.slots.close();
    }
}
