use trogon_sandbox_contract::LeaseId;

use crate::locked::locked;
use crate::{CustodyPhase, KubeVirtProvider, KubeVirtProviderError, LeaseJournal};

impl KubeVirtProvider {
    /// Ends `lease_id` with the lease's gate held: deletes exactly the resources its custody names, then settles
    /// the record. A failed delete keeps what remains for the next attempt.
    pub(crate) async fn cancel_locked(&self, lease_id: &LeaseId) -> Result<(), KubeVirtProviderError> {
        self.release(lease_id).await?;
        if let Some(keys) = self.ssh_keys.get() {
            keys.forget_lease(lease_id).await;
        }
        Ok(())
    }

    async fn release(&self, lease_id: &LeaseId) -> Result<(), KubeVirtProviderError> {
        let Some(record) = self.journal.get(lease_id)? else {
            if self.holds_any(lease_id).await {
                return Err(KubeVirtProviderError::CustodyMissing);
            }
            return Ok(());
        };
        let journal = LeaseJournal::handle(&self.journal, &record);
        let resources = self.held_resources(lease_id).await.unwrap_or(record.resources);
        let ops = self.ops.with_durable_receipts(journal.clone(), resources.clone());
        if let Err(error) = ops.release_resources(&resources).await {
            self.retained
                .lock()
                .await
                .insert(lease_id.clone(), ops.resource_receipts());
            return Err(error.into());
        }
        journal.set_phase(CustodyPhase::Cleaned)?;
        self.forget(lease_id).await;
        Ok(journal.finish()?)
    }

    /// The resources memory holds for `lease_id`, preferring what an interrupted allocation, then a failed cleanup,
    /// last recorded.
    async fn held_resources(&self, lease_id: &LeaseId) -> Option<crate::LeaseResources> {
        let active = self
            .active
            .lock()
            .await
            .get(lease_id)
            .map(|active| active.resources.clone());
        let retained = self.retained.lock().await.get(lease_id).cloned();
        self.in_flight_resources(lease_id).or(retained).or(active)
    }

    async fn forget(&self, lease_id: &LeaseId) {
        self.active.lock().await.remove(lease_id);
        self.retained.lock().await.remove(lease_id);
        locked(&self.in_flight).remove(lease_id);
    }
}
