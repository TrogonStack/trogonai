use std::sync::Arc;

use async_nats::jetstream;
use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload,
    ProviderKind, ProviderLease,
};
use trogon_sandbox_protocol::AttemptLedger;

use crate::lease_dispatch::with_lifecycle;
use crate::{VercelDispatchError, VercelProviderError, VercelWorkerProvider};

impl VercelWorkerProvider {
    /// Keeps targeted dispatches in `ledger`, so a repeat of an attempt gets its first receipt and a cleanup is
    /// confirmed only once the sandbox is gone. Binds the provider to the ledger's worker id.
    pub fn configure_dispatch_ledger(&self, ledger: Arc<AttemptLedger>) -> Result<(), VercelDispatchError> {
        self.bind_worker(ledger.worker_id())?;
        self.ledger
            .set(ledger)
            .map_err(|_| VercelDispatchError::AlreadyConfigured)
    }

    /// The ledger targeted dispatches are kept in, when the worker keeps one.
    pub fn dispatch_ledger(&self) -> Option<&Arc<AttemptLedger>> {
        self.ledger.get()
    }

    /// Leases the sandbox a targeted dispatch names, once per attempt. A repeat gets the first receipt, and an
    /// attempt that was cleaned up is refused. `lifecycle` is where the lease's lifecycle events go, if anywhere.
    pub async fn lease_dispatch(
        &self,
        dispatch: &ProviderDispatchPayload,
        lifecycle: Option<&jetstream::Context>,
    ) -> Result<ProviderLease, VercelDispatchError> {
        let ledger = self.dispatch_ledger().ok_or(VercelDispatchError::NotConfigured)?;
        let target = dispatch.target.clone().ok_or(VercelDispatchError::Untargeted)?;
        if &target.worker_id != ledger.worker_id() {
            return Err(VercelDispatchError::TargetMismatch);
        }
        let identity = LeaseAttemptIdentity {
            lease_id: dispatch.lease_id.clone(),
            provider: ProviderKind::Vercel,
            target,
        };
        let allocate = || async {
            let mut lease = self.launch(&dispatch.request, &dispatch.lease_id).await?;
            lease.epoch = identity.target.epoch;
            Ok::<_, VercelProviderError>(lease)
        };
        let receipt = ledger
            .dispatch(&identity, || async {
                match lifecycle {
                    Some(js) => with_lifecycle(&dispatch.lease_id, js, allocate()).await,
                    None => allocate().await,
                }
            })
            .await?;
        Ok(receipt)
    }

    /// Deletes the sandbox a targeted cleanup names and records the result. The sandbox is found by the attempt's
    /// receipt, so a cleanup still works after a restart lost the worker's own record of the lease.
    pub async fn cleanup_dispatch(&self, request: &ProviderCleanupRequest) -> ProviderCleanupResponse {
        let failed = |error: VercelDispatchError| ProviderCleanupResponse::Failed {
            identity: request.identity.clone(),
            message: error.to_string(),
        };
        let Some(ledger) = self.dispatch_ledger() else {
            return failed(VercelDispatchError::NotConfigured);
        };
        if request.identity.provider != ProviderKind::Vercel || &request.identity.target.worker_id != ledger.worker_id()
        {
            return failed(VercelDispatchError::TargetMismatch);
        }
        ledger
            .cleanup(request, |state| async move {
                if state.backend_cleaned {
                    return Ok(());
                }
                let receipt = state.receipt.ok_or(VercelDispatchError::CleanupWithoutReceipt)?;
                self.release_exact(&state.identity.lease_id, receipt.sandbox_id.as_str())
                    .await
            })
            .await
    }

    /// Deletes the sandbox `name` for `lease_id` and drops the lease if this worker still holds it. A sandbox that is
    /// already gone counts as deleted.
    async fn release_exact(&self, lease_id: &LeaseId, name: &str) -> Result<(), VercelDispatchError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let held = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .map(|lease| lease.name.clone());
        if held.as_deref().is_some_and(|held| held != name) {
            return Err(VercelDispatchError::ForeignSandbox);
        }
        self.api.delete_sandbox(name).await?;
        if held.is_some() {
            self.active.lock().await.remove(lease_id.as_str());
            self.exec.forget_lease(lease_id.as_str()).await;
        }
        Ok(())
    }
}
