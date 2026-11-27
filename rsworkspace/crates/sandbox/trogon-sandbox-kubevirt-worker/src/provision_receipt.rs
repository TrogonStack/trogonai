use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tracing::warn;
use trogon_sandbox_contract::LeaseId;

use crate::locked::locked;
use crate::{CustodyError, CustodyPhase, LeaseJournal, LeaseResources};

/// Settles the custody of an allocation that ends without reaching the ready phase, even when its future is dropped:
/// the receipts gathered so far are written down and the lease moves to cleanup, or to unresolved while a create is
/// in doubt.
pub(crate) struct ProvisionReceipt {
    pub(crate) lease: LeaseId,
    pub(crate) preparing: Arc<Mutex<HashSet<LeaseId>>>,
    pub(crate) journal: Arc<LeaseJournal>,
    pub(crate) resources: Arc<Mutex<LeaseResources>>,
}

impl ProvisionReceipt {
    fn settle(&self) -> Result<(), CustodyError> {
        if self.journal.record()?.phase == CustodyPhase::Ready {
            return Ok(());
        }
        self.journal.resources(&locked(&self.resources))?;
        let unresolved = self.journal.record()?.create.is_some();
        self.journal.set_phase(if unresolved {
            CustodyPhase::Unresolved
        } else {
            CustodyPhase::Cleanup
        })
    }
}

impl Drop for ProvisionReceipt {
    fn drop(&mut self) {
        if let Err(error) = self.settle() {
            warn!(lease_id = %self.lease, %error, "KubeVirt custody remains pending durable reconciliation");
        }
        locked(&self.preparing).remove(&self.lease);
    }
}
