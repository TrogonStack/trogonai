use std::sync::Arc;

use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload,
    ProviderKind, ProviderLease, SandboxPhase,
};
use trogon_sandbox_protocol::{AttemptLedger, run_phase};

use crate::lease_dispatch::with_lifecycle;
use crate::{DaytonaDispatchError, DaytonaProviderError, DaytonaWorkerProvider};

impl DaytonaWorkerProvider {
    /// Keeps targeted dispatches in `ledger`, so a repeat of an attempt gets its first receipt and a cleanup is
    /// confirmed only once the sandbox is gone. Binds the provider to the ledger's worker id.
    pub fn configure_dispatch_ledger(&self, ledger: Arc<AttemptLedger>) -> Result<(), DaytonaDispatchError> {
        self.bind_worker(ledger.worker_id())?;
        self.ledger
            .set(ledger)
            .map_err(|_| DaytonaDispatchError::AlreadyConfigured)
    }

    /// The ledger targeted dispatches are kept in, when the worker keeps one.
    pub fn dispatch_ledger(&self) -> Option<&Arc<AttemptLedger>> {
        self.ledger.get()
    }

    /// Leases the sandbox a targeted dispatch names, once per attempt. A repeat gets the first receipt, and an
    /// attempt that was cleaned up is refused. The lease's lifecycle events go where the provider publishes them.
    pub async fn lease_dispatch(
        &self,
        dispatch: &ProviderDispatchPayload,
    ) -> Result<ProviderLease, DaytonaDispatchError> {
        let ledger = self.dispatch_ledger().ok_or(DaytonaDispatchError::NotConfigured)?;
        let target = dispatch.target.clone().ok_or(DaytonaDispatchError::Untargeted)?;
        if &target.worker_id != ledger.worker_id() {
            return Err(DaytonaDispatchError::TargetMismatch);
        }
        let identity = LeaseAttemptIdentity {
            lease_id: dispatch.lease_id.clone(),
            provider: ProviderKind::Daytona,
            target,
        };
        let allocate = || async {
            let mut lease = self.launch(&dispatch.request, &dispatch.lease_id).await?;
            lease.epoch = identity.target.epoch;
            Ok::<_, DaytonaProviderError>(lease)
        };
        let receipt = ledger
            .dispatch(&identity, || async {
                match &self.lifecycle {
                    Some(js) => with_lifecycle(&dispatch.lease_id, js, allocate()).await,
                    None => allocate().await,
                }
            })
            .await?;
        Ok(receipt)
    }

    /// Deletes the sandbox a targeted cleanup names, with the native snapshots it still owns, and records the result.
    /// The sandbox is found by the attempt's receipt, so a cleanup still works after a restart lost the worker's own
    /// record of the lease.
    pub async fn cleanup_dispatch(&self, request: &ProviderCleanupRequest) -> ProviderCleanupResponse {
        let failed = |error: DaytonaDispatchError| ProviderCleanupResponse::Failed {
            identity: request.identity.clone(),
            message: error.to_string(),
        };
        let Some(ledger) = self.dispatch_ledger() else {
            return failed(DaytonaDispatchError::NotConfigured);
        };
        if request.identity.provider != ProviderKind::Daytona
            || &request.identity.target.worker_id != ledger.worker_id()
        {
            return failed(DaytonaDispatchError::TargetMismatch);
        }
        ledger
            .cleanup(request, |state| async move {
                if state.backend_cleaned {
                    return Ok(());
                }
                let receipt = state.receipt.ok_or(DaytonaDispatchError::CleanupWithoutReceipt)?;
                self.release_exact(&state.identity.lease_id, receipt.sandbox_id.as_str())
                    .await
            })
            .await
    }

    /// Deletes the native snapshots `sandbox` still owns, then `sandbox` itself, and drops the lease if this worker
    /// still holds it. A sandbox that is already gone counts as deleted.
    async fn release_exact(&self, lease_id: &LeaseId, sandbox: &str) -> Result<(), DaytonaDispatchError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let held = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .map(|lease| lease.id.clone());
        if held.as_deref().is_some_and(|held| held != sandbox) {
            return Err(DaytonaDispatchError::ForeignSandbox);
        }
        let api = self.clean_native_snapshots(sandbox).await?;
        let emitter = self.emitter(lease_id);
        run_phase(emitter.as_ref(), SandboxPhase::Reclaim, None, || async {
            api.delete_sandbox(sandbox).await.map(drop)
        })
        .await?;
        if held.is_some() {
            self.active.lock().await.remove(lease_id.as_str());
            self.exec.forget_lease(lease_id.as_str()).await;
        }
        Ok(())
    }
}
