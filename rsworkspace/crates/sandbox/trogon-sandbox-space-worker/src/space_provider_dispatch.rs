use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload,
    ProviderKind, ProviderLease,
};
use trogon_sandbox_protocol::{AttemptPhase, CleanupState};

use crate::{LeaseViolationError, SpaceId, SpaceProvider, SpaceProviderError};

impl SpaceProvider {
    /// Leases for a dispatch. A dispatch that names a target is recorded in the dispatch
    /// ledger first, so a crash before the reply still leaves enough to find the sandbox.
    pub async fn lease_dispatch(
        &self,
        dispatch: &ProviderDispatchPayload,
    ) -> Result<ProviderLease, SpaceProviderError> {
        let request = &dispatch.request;
        if !(dispatch.resolved_drives.is_empty() && request.spec.drives().is_empty()) {
            return Err(LeaseViolationError::Volumes.into());
        }
        self.validate_lease(request)?;
        let Some(target) = dispatch.target.clone() else {
            return self.lease_inner(request, &dispatch.lease_id, None).await;
        };
        let identity = LeaseAttemptIdentity {
            lease_id: dispatch.lease_id.clone(),
            provider: ProviderKind::Space,
            target,
        };
        Ok(self
            .ledger
            .dispatch(&identity, || {
                self.lease_inner(request, &dispatch.lease_id, Some(&identity))
            })
            .await?)
    }

    /// Deletes the sandbox of a dispatch attempt and records the result.
    pub async fn cleanup_dispatch(&self, request: &ProviderCleanupRequest) -> ProviderCleanupResponse {
        self.ledger.cleanup(request, |state| self.teardown(state)).await
    }

    async fn teardown(&self, state: CleanupState) -> Result<(), SpaceProviderError> {
        if state.backend_cleaned {
            return Ok(());
        }
        let lease = state.identity.lease_id.clone();
        self.abort_pending(&lease).await?;
        let id = match &state.receipt {
            Some(receipt) => SpaceId::parse(receipt.sandbox_id.as_str())?,
            None => self.find_by_dispatch(&state.identity).await?,
        };
        if let Some(active) = self.registry.lock().await.active.get(lease.as_str())
            && active.sandbox_id != id
        {
            return Err(SpaceProviderError::ForeignAttempt);
        }
        self.destroy(&id).await?;
        self.registry.lock().await.active.remove(lease.as_str());
        Ok(())
    }

    /// The one sandbox tagged with the attempt, for a create whose reply was lost.
    async fn find_by_dispatch(&self, identity: &LeaseAttemptIdentity) -> Result<SpaceId, SpaceProviderError> {
        let mut matches = self.owned_sandboxes().await?.into_iter().filter(|sandbox| {
            sandbox.metadata_text("sandbox_dispatch_attempt") == Some(identity.target.attempt_id.as_str())
                && sandbox.metadata_text("sandbox_lease") == Some(identity.lease_id.as_str())
        });
        match (matches.next(), matches.next()) {
            (Some(only), None) => Ok(only.id),
            _ => Err(SpaceProviderError::AmbiguousCreate),
        }
    }

    /// Cancels an unfinished create and waits for it to wind down.
    pub(crate) async fn abort_pending(&self, lease: &LeaseId) -> Result<(), SpaceProviderError> {
        let pending = self.registry.lock().await.pending.get(lease.as_str()).cloned();
        let Some(pending) = pending else {
            return Ok(());
        };
        pending.cancel();
        if let Some(id) = pending.sandbox() {
            self.destroy(&id).await?;
        }
        let mut done = pending.finished();
        let wait = self.config.boot_timeout + self.config.operation_timeout;
        match tokio::time::timeout(wait, done.wait_for(|finished| *finished)).await {
            Ok(Ok(_)) => Ok(()),
            _ => Err(SpaceProviderError::AbortTimeout),
        }
    }

    /// Deletes the lease's sandbox. An unfinished create is told to stop and to delete
    /// what it made, so a cancel that races a create always ends with the sandbox gone.
    pub(crate) async fn cancel_lease(&self, lease: &LeaseId) -> Result<(), SpaceProviderError> {
        self.abort_pending(lease).await?;
        let record = self.registry.lock().await.active.get(lease.as_str()).cloned();
        let Some(record) = record else {
            return Ok(());
        };
        self.destroy(&record.sandbox_id).await?;
        self.registry.lock().await.active.remove(lease.as_str());
        Ok(())
    }

    /// Deletes every sandbox this worker owns, which is how a restart reclaims what a
    /// crashed run left behind. Returns how many it deleted.
    pub async fn recover(&self) -> Result<usize, SpaceProviderError> {
        let mut count = 0;
        let states = self.ledger.try_states().await?;
        for sandbox in self.owned_sandboxes().await? {
            let attempts = states.iter().filter(|state| {
                sandbox.metadata_text("sandbox_dispatch_attempt") == Some(state.identity.target.attempt_id.as_str())
                    && sandbox.metadata_text("sandbox_lease") == Some(state.identity.lease_id.as_str())
                    && state.phase != AttemptPhase::Cleaned
            });
            for state in attempts {
                let receipt = Self::receipt(&sandbox.id, &state.identity.lease_id);
                self.ledger.record_backend_receipt(&state.identity, receipt).await?;
            }
            self.destroy(&sandbox.id).await?;
            count += 1;
        }
        self.registry.lock().await.active.clear();
        Ok(count)
    }

    /// Releases everything the worker holds before it exits.
    pub async fn shutdown(&self) -> Result<(), SpaceProviderError> {
        self.recover().await.map(|_| ())
    }
}
