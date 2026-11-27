//! Serializes work on one lease, so a capture and a release never interleave.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use tokio::sync::{Mutex, OwnedMutexGuard};
use trogon_sandbox_contract::LeaseId;

#[derive(Default)]
pub struct LeaseGates(StdMutex<HashMap<String, Weak<Mutex<()>>>>);

impl LeaseGates {
    fn gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease.as_str()).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease.as_str().to_owned(), Arc::downgrade(&gate));
        gate
    }
    pub async fn lock(&self, lease: &LeaseId) -> OwnedMutexGuard<()> {
        self.gate(lease).lock_owned().await
    }
    pub fn try_lock(&self, lease: &LeaseId) -> Option<OwnedMutexGuard<()>> {
        self.gate(lease).try_lock_owned().ok()
    }
}
