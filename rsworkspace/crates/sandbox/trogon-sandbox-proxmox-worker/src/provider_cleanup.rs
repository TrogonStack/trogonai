use trogon_sandbox_contract::{LeaseAttemptIdentity, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse};
use trogon_sandbox_protocol::CleanupState;

use crate::check::ensure;
use crate::provider_journal::read_journal;
use crate::{LeaseRecord, ProxmoxProvider, ProxmoxProviderError, VmPhase};

impl ProxmoxProvider {
    /// Retries one VM whose cleanup failed earlier, unless a lease operation is already running.
    pub(crate) async fn reap_pending(&self) {
        let Ok(_allocation) = self.allocation.try_lock() else {
            return;
        };
        let Some(lease) = self.pending_cleanup.lock().await.pop_front() else {
            return;
        };
        if let Err(error) = self.cleanup_lease(&lease).await {
            tracing::warn!(%error, "Proxmox cleanup will be retried");
            self.pending_cleanup.lock().await.push_back(lease);
        }
    }

    /// How many leases this worker holds, after retrying one VM whose cleanup failed earlier.
    pub async fn current_leases(&self) -> usize {
        self.reap_pending().await;
        self.active.lock().await.len()
    }

    async fn cleanup_lease(&self, lease: &LeaseId) -> Result<(), ProxmoxProviderError> {
        let held = self.active.lock().await.get(lease).cloned();
        let record = match held {
            Some(record) => record,
            None => match read_journal(&self.journal_path(lease)?).await? {
                Some(record) => record,
                None => return Ok(()),
            },
        };
        let record = self.latest_record(record).await?;
        if let Some(proxy) = self.proxies.lock().await.remove(lease)
            && let Err(error) = proxy.shutdown().await
        {
            tracing::warn!(%error, "proxy stopped with an error");
        }
        self.destroy_owned(lease, &record).await?;
        self.remove_journal(lease).await?;
        self.active.lock().await.remove(lease);
        Ok(())
    }

    pub(crate) async fn cancel_current(&self, lease: &LeaseId) -> Result<(), ProxmoxProviderError> {
        let result = self.cleanup_lease(lease).await;
        let mut pending = self.pending_cleanup.lock().await;
        pending.retain(|id| id != lease);
        if result.is_err() {
            pending.push_back(lease.clone());
        }
        result
    }

    /// Ends the lease: its VM is destroyed. A lease this worker has no record of is already ended.
    pub async fn release(&self, lease: &LeaseId) -> Result<(), ProxmoxProviderError> {
        let _allocation = self.allocation.lock().await;
        self.cancel_current(lease).await
    }

    async fn cleanup_attempt(&self, state: &CleanupState) -> Result<(), ProxmoxProviderError> {
        let _allocation = self.allocation.lock().await;
        if state.backend_cleaned {
            return Ok(());
        }
        let identity = &state.identity;
        let record = read_journal(&self.attempt_path(identity))
            .await?
            .ok_or(ProxmoxProviderError::Backend(
                "exact VM allocation journal is unavailable",
            ))?;
        ensure(
            record.assignment.as_ref() == Some(identity),
            ProxmoxProviderError::DifferentAttempt,
        )?;
        let receipt_matches = state
            .receipt
            .as_ref()
            .is_none_or(|receipt| receipt.sandbox_id.as_str() == record.vm.to_string());
        ensure(
            receipt_matches,
            ProxmoxProviderError::Backend("VM allocation journal differs from the backend receipt"),
        )?;
        if matches!(record.phase, VmPhase::Destroyed) {
            return self.settle_destroyed_local(identity, &record).await;
        }
        let held = self.active.lock().await.get(&identity.lease_id).cloned();
        let current =
            match held {
                Some(record) => record,
                None => read_journal(&self.journal_path(&identity.lease_id)?).await?.ok_or(
                    ProxmoxProviderError::Backend("current VM allocation ownership is unavailable"),
                )?,
            };
        ensure(current.same_allocation(&record), ProxmoxProviderError::DifferentAttempt)?;
        self.cancel_current(&identity.lease_id).await
    }

    async fn settle_destroyed_local(
        &self,
        identity: &LeaseAttemptIdentity,
        record: &LeaseRecord,
    ) -> Result<(), ProxmoxProviderError> {
        let active = self.active.lock().await.get(&identity.lease_id).cloned();
        let journal = read_journal(&self.journal_path(&identity.lease_id)?).await?;
        let same = |candidate: &LeaseRecord| candidate.same_allocation(record);
        let has_original = active.as_ref().is_some_and(same) || journal.as_ref().is_some_and(same);
        if has_original {
            ensure(
                active.as_ref().is_none_or(same) && journal.as_ref().is_none_or(same),
                ProxmoxProviderError::Backend("VM cleanup local ownership is inconsistent"),
            )?;
            self.cancel_current(&identity.lease_id).await?;
        } else if active.is_none() && journal.is_none() {
            self.pending_cleanup
                .lock()
                .await
                .retain(|lease| lease != &identity.lease_id);
        }
        Ok(())
    }

    async fn forget_cleaned_attempt(&self, identity: &LeaseAttemptIdentity) -> Result<(), ProxmoxProviderError> {
        let _allocation = self.allocation.lock().await;
        let path = self.attempt_path(identity);
        let Some(record) = read_journal(&path).await? else {
            return Ok(());
        };
        ensure(
            record.assignment.as_ref() == Some(identity) && matches!(record.phase, VmPhase::Destroyed),
            ProxmoxProviderError::Backend("completed VM allocation proof is unconfirmed"),
        )?;
        self.settle_destroyed_local(identity, &record).await?;
        self.remove_attempt_journal(&path).await
    }

    /// Tears down the attempt a cleanup request names, wherever its VM is.
    pub async fn cleanup_dispatch(&self, request: &ProviderCleanupRequest) -> ProviderCleanupResponse {
        let response = self
            .ledger
            .cleanup(request, |state| async move { self.cleanup_attempt(&state).await })
            .await;
        if matches!(response, ProviderCleanupResponse::Cleaned { .. })
            && let Err(error) = self.forget_cleaned_attempt(&request.identity).await
        {
            tracing::warn!(%error, "completed VM allocation proof remains pending removal");
        }
        response
    }
}
