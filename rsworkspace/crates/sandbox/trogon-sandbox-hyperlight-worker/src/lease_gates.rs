use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::sync::Mutex as AsyncMutex;
use trogon_sandbox_contract::LeaseId;

/// One async gate per lease, so lease-changing work on the same lease runs one at a time.
///
/// A gate lives as long as some caller holds it; dropped gates are pruned on the next request.
#[derive(Default)]
pub(crate) struct LeaseGates(Mutex<HashMap<LeaseId, Weak<AsyncMutex<()>>>>);

impl LeaseGates {
    pub(crate) fn gate(&self, lease: &LeaseId) -> Arc<AsyncMutex<()>> {
        let mut gates = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(lease.clone(), Arc::downgrade(&gate));
        gate
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

#[cfg(test)]
mod tests;
