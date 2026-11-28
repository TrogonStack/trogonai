use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, watch};
use trogon_sandbox_contract::{
    LeaseError, LeaseId, NativeHandle, NativeHandleResponse, NativeSnapshotId, NativeSnapshotReceipt,
    NativeSnapshotRef, ProviderKind, SandboxFreezeResponse, SnapshotExportError, SnapshotExportOutcome, WorkerId,
};
use trogon_sandbox_protocol::AttemptLedger;

use crate::active_lease::{ActiveLease, route_map};
use crate::{LeaseSandbox, VercelApi, VercelApiError, VercelExec, VercelPlan, VercelProviderError};

/// A [`Provider`](trogon_sandbox_protocol::Provider) that leases sandboxes from Vercel.
pub struct VercelWorkerProvider {
    pub(crate) api: VercelApi,
    pub(crate) default_runtime: String,
    pub(crate) max_vcpus: u32,
    pub(crate) plan: VercelPlan,
    pub(crate) exec: Arc<VercelExec>,
    pub(crate) active: Arc<Mutex<HashMap<String, ActiveLease>>>,
    gates: std::sync::Mutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
    pub(crate) worker_id: std::sync::OnceLock<WorkerId>,
    pub(crate) ledger: std::sync::OnceLock<Arc<AttemptLedger>>,
    ready: watch::Sender<bool>,
}

impl VercelWorkerProvider {
    pub fn new(
        api: VercelApi,
        default_runtime: impl Into<String>,
        max_vcpus: u32,
        plan: VercelPlan,
        exec: Arc<VercelExec>,
    ) -> Self {
        Self {
            api,
            default_runtime: default_runtime.into(),
            max_vcpus: max_vcpus.min(plan.max_vcpus()),
            plan,
            exec,
            active: Arc::new(Mutex::new(HashMap::new())),
            gates: std::sync::Mutex::new(HashMap::new()),
            worker_id: std::sync::OnceLock::new(),
            ledger: std::sync::OnceLock::new(),
            ready: watch::channel(false).0,
        }
    }

    /// Binds the provider to the worker id it reports in leases. Binding the same id again is fine.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), VercelProviderError> {
        if self.worker_id.get_or_init(|| worker.clone()) != worker {
            return Err(VercelProviderError::BoundToAnotherWorker);
        }
        Ok(())
    }

    /// Marks the worker as serving every subject it announces, so heartbeats may start.
    pub(crate) fn mark_ready(&self) {
        self.ready.send_replace(true);
    }

    /// Waits until the worker serves every subject it announces.
    pub async fn wait_ready(&self) {
        let mut ready = self.ready.subscribe();
        let _ = ready.wait_for(|ready| *ready).await;
    }

    pub fn exec(&self) -> &Arc<VercelExec> {
        &self.exec
    }

    pub(crate) fn lease_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease.clone(), Arc::downgrade(&gate));
        gate
    }

    /// Snapshots the sandbox a lease holds through Vercel. Vercel ends the session once the snapshot exists, so the
    /// lease needs a resume before it runs anything again.
    pub async fn native_snapshot(&self, session_id: &str) -> SnapshotExportOutcome {
        match self.api.snapshot_session(session_id, None).await {
            Ok(snap) if snap.status == "failed" => SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                message: format!("Vercel snapshot {} failed", snap.id),
            }),
            Ok(snap) => match NativeSnapshotId::new(snap.id) {
                Ok(id) => SnapshotExportOutcome::Native(NativeSnapshotReceipt {
                    reference: NativeSnapshotRef {
                        provider: ProviderKind::Vercel,
                        id,
                        expires_at: snap.expires_at.map(|ms| ms / 1000),
                    },
                    bytes: snap.size_bytes,
                }),
                Err(error) => SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                    message: format!("Vercel named the snapshot with an id the protocol refuses: {error}"),
                }),
            },
            Err(
                e @ (VercelApiError::Unauthorized(_) | VercelApiError::NotFound(_) | VercelApiError::Rejected { .. }),
            ) => SnapshotExportOutcome::Err(SnapshotExportError::Permanent { message: e.to_string() }),
            Err(e) => SnapshotExportOutcome::Err(SnapshotExportError::Transient { message: e.to_string() }),
        }
    }

    /// The vendor handle for the sandbox `lease_id` holds on this worker.
    pub async fn native_handle_for_lease(&self, lease_id: &LeaseId) -> NativeHandleResponse {
        let active = self.active.lock().await;
        match active.get(lease_id.as_str()) {
            Some(lease) => NativeHandleResponse::Available(NativeHandle {
                provider: ProviderKind::Vercel,
                vendor_id: lease.name.clone(),
                url: None,
                region: lease.region.clone(),
            }),
            None => NativeHandleResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            }),
        }
    }

    /// The sandbox a lease holds on this worker, or `None` when another worker holds it. Waits out an in-flight
    /// snapshot and retries a resume that failed after one.
    pub async fn sandbox_for_lease(&self, lease_id: &LeaseId) -> Option<LeaseSandbox> {
        let gate = Arc::clone(&self.active.lock().await.get(lease_id.as_str())?.gate);
        let _held = gate.lock().await;
        let (stopped, name) = {
            let active = self.active.lock().await;
            let lease = active.get(lease_id.as_str())?;
            (lease.stopped, lease.name.clone())
        };
        if stopped && let Err(e) = self.adopt_new_session(lease_id, &name).await {
            tracing::warn!(
                lease_id = %lease_id,
                error = %e,
                "the sandbox session ended after a snapshot and could not be resumed; operations will fail until a \
                 wake succeeds"
            );
        }
        let active = self.active.lock().await;
        let lease = active.get(lease_id.as_str())?;
        Some(LeaseSandbox {
            name: lease.name.clone(),
            session_id: lease.session_id.clone(),
            routes: lease.routes.clone(),
        })
    }

    /// Starts a new session for the named sandbox and records it on the lease.
    pub(crate) async fn adopt_new_session(&self, lease_id: &LeaseId, name: &str) -> Result<(), VercelApiError> {
        let resumed = self.api.get_sandbox(name, true).await?;
        if let Some(lease) = self.active.lock().await.get_mut(lease_id.as_str()) {
            lease.session_id = resumed.session.id;
            lease.routes = route_map(&resumed.routes);
            lease.stopped = false;
        }
        Ok(())
    }

    /// Snapshots the sandbox `lease_id` holds, then starts a new session so the lease stays usable. The receipt is
    /// returned even when the resume fails; the lease is then marked stopped and the next operation or wake retries
    /// the resume.
    pub async fn snapshot_lease(&self, lease_id: &LeaseId) -> SnapshotExportOutcome {
        let not_held = || {
            SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
                message: format!("lease {lease_id} is not held by this worker"),
            })
        };
        let gate = match self.active.lock().await.get(lease_id.as_str()) {
            Some(lease) => Arc::clone(&lease.gate),
            None => return not_held(),
        };
        let _held = gate.lock().await;
        let (name, session_id) = {
            let active = self.active.lock().await;
            match active.get(lease_id.as_str()) {
                Some(lease) => (lease.name.clone(), lease.session_id.clone()),
                None => return not_held(),
            }
        };
        let outcome = self.native_snapshot(&session_id).await;
        if !matches!(outcome, SnapshotExportOutcome::Native(_)) {
            return outcome;
        }
        if let Some(lease) = self.active.lock().await.get_mut(lease_id.as_str()) {
            lease.stopped = true;
        }
        if let Err(e) = self.adopt_new_session(lease_id, &name).await {
            tracing::warn!(
                lease_id = %lease_id,
                error = %e,
                "snapshot taken but the sandbox could not be resumed"
            );
        }
        outcome
    }

    /// Stops (`pause`) or restarts (`!pause`) the sandbox `lease_id` holds. Idempotent. A stopped persistent sandbox
    /// keeps its files; its processes do not survive.
    pub async fn set_paused_for_lease(&self, lease_id: &LeaseId, pause: bool) -> SandboxFreezeResponse {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let (name, session_id, persistent) = {
            let active = self.active.lock().await;
            match active.get(lease_id.as_str()) {
                Some(lease) => (lease.name.clone(), lease.session_id.clone(), lease.persistent),
                None => {
                    return SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
                        lease_id: lease_id.clone(),
                    });
                }
            }
        };
        if pause {
            if !persistent {
                return SandboxFreezeResponse::Denied(LeaseError::InvalidSpec {
                    reason: "the sandbox was created with persistent=false, so a stop would lose its files".to_owned(),
                });
            }
            return match self.api.stop_session(&session_id).await {
                Ok(()) => SandboxFreezeResponse::Done,
                // Already stopped.
                Err(VercelApiError::Conflict(_)) => SandboxFreezeResponse::Done,
                Err(e) => SandboxFreezeResponse::Denied(LeaseError::Backend { message: e.to_string() }),
            };
        }
        match self.adopt_new_session(lease_id, &name).await {
            Ok(()) => SandboxFreezeResponse::Done,
            Err(e) => SandboxFreezeResponse::Denied(LeaseError::Backend { message: e.to_string() }),
        }
    }

    pub async fn active_lease_count(&self) -> usize {
        self.active.lock().await.len()
    }
}
