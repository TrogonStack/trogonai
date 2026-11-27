//! Makes a worker's lease dispatch idempotent.
//!
//! The broker redelivers a dispatch when a reply is lost, so a worker can see the same lease id twice.
//! [`RedispatchGuard`] runs the launch once per lease id and hands every repeat the grant of the first, so a repeat
//! never starts a second sandbox. A repeat that arrives while the first launch is still running waits for it.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::OnceCell;
use trogon_sandbox_contract::{LeaseId, ProviderLease};

/// Runs each lease's launch at most once and remembers its grant.
#[derive(Default)]
pub struct RedispatchGuard {
    grants: Mutex<HashMap<String, Arc<OnceCell<ProviderLease>>>>,
}

impl RedispatchGuard {
    /// A guard that remembers no grant.
    pub fn new() -> Self {
        Self::default()
    }

    fn grants(&self) -> MutexGuard<'_, HashMap<String, Arc<OnceCell<ProviderLease>>>> {
        self.grants.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Runs `launch` unless this lease id already has a grant, and returns the grant. A failed launch is not
    /// remembered, so the broker's retry runs it again.
    pub async fn get_or_launch<F, Fut, E>(&self, lease_id: &LeaseId, launch: F) -> Result<ProviderLease, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<ProviderLease, E>>,
    {
        let cell = Arc::clone(self.grants().entry(lease_id.as_str().to_owned()).or_default());
        let result = cell.get_or_try_init(launch).await.cloned();
        if result.is_err() {
            let mut grants = self.grants();
            if grants
                .get(lease_id.as_str())
                .is_some_and(|held| Arc::ptr_eq(held, &cell) && held.get().is_none())
            {
                grants.remove(lease_id.as_str());
            }
        }
        result
    }

    /// Forgets a lease's grant once it is released, so a reused id launches afresh.
    pub fn forget(&self, lease_id: &LeaseId) {
        self.grants().remove(lease_id.as_str());
    }
}
