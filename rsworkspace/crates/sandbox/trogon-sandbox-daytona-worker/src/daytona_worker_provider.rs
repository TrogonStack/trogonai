use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, NativeHandle, NativeHandleResponse, ProviderKind, SandboxFreezeResponse, WorkerId,
};

use crate::active_lease::ActiveLease;
use crate::constants::{DEFAULT_START_TIMEOUT, STATE_POLL_INTERVAL};
use crate::settle::settle;
use crate::{DaytonaApi, DaytonaExec, DaytonaProviderError, LeaseSandbox, SandboxState, Wait};

/// A [`Provider`](trogon_sandbox_protocol::Provider) that leases sandboxes from Daytona.
pub struct DaytonaWorkerProvider {
    pub(crate) api: DaytonaApi,
    pub(crate) default_snapshot: String,
    pub(crate) exec: Arc<DaytonaExec>,
    pub(crate) active: Arc<Mutex<HashMap<String, ActiveLease>>>,
    gates: std::sync::Mutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
    pub(crate) worker_id: std::sync::OnceLock<WorkerId>,
    pub(crate) wait: Wait,
}

impl DaytonaWorkerProvider {
    pub fn new(api: DaytonaApi, default_snapshot: impl Into<String>, exec: Arc<DaytonaExec>) -> Self {
        Self {
            api,
            default_snapshot: default_snapshot.into(),
            exec,
            active: Arc::new(Mutex::new(HashMap::new())),
            gates: std::sync::Mutex::new(HashMap::new()),
            worker_id: std::sync::OnceLock::new(),
            wait: Wait {
                timeout: DEFAULT_START_TIMEOUT,
                interval: STATE_POLL_INTERVAL,
            },
        }
    }

    /// Changes how long the worker waits for a sandbox to reach the state a lease needs, and how often it looks.
    #[must_use]
    pub fn with_wait(mut self, wait: Wait) -> Self {
        self.wait = wait;
        self
    }

    /// Binds the provider to the worker id it reports in leases. Binding the same id again is fine.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), DaytonaProviderError> {
        if self.worker_id.get_or_init(|| worker.clone()) != worker {
            return Err(DaytonaProviderError::BoundToAnotherWorker);
        }
        Ok(())
    }

    pub fn exec(&self) -> &Arc<DaytonaExec> {
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

    /// The vendor handle for the sandbox `lease_id` holds on this worker.
    pub async fn native_handle_for_lease(&self, lease_id: &LeaseId) -> NativeHandleResponse {
        let active = self.active.lock().await;
        match active.get(lease_id.as_str()) {
            Some(lease) => NativeHandleResponse::Available(NativeHandle {
                provider: ProviderKind::Daytona,
                vendor_id: lease.id.clone(),
                url: None,
                region: lease.region.clone(),
            }),
            None => NativeHandleResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            }),
        }
    }

    /// The sandbox a lease holds on this worker, or `None` when another worker holds it.
    pub async fn sandbox_for_lease(&self, lease_id: &LeaseId) -> Option<LeaseSandbox> {
        let active = self.active.lock().await;
        active
            .get(lease_id.as_str())
            .map(|lease| LeaseSandbox { id: lease.id.clone() })
    }

    /// Stops (`pause`) or starts (`!pause`) the sandbox `lease_id` holds. Idempotent. A stopped sandbox keeps its
    /// files; its processes do not survive.
    pub async fn set_paused_for_lease(&self, lease_id: &LeaseId, pause: bool) -> SandboxFreezeResponse {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        let id = match self.active.lock().await.get(lease_id.as_str()) {
            Some(lease) => lease.id.clone(),
            None => {
                return SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound {
                    lease_id: lease_id.clone(),
                });
            }
        };
        let target = if pause {
            SandboxState::Stopped
        } else {
            SandboxState::Started
        };
        match settle(&self.api, &id, target, self.wait).await {
            Ok(()) => SandboxFreezeResponse::Done,
            Err(error) => SandboxFreezeResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            }),
        }
    }

    pub async fn active_lease_count(&self) -> usize {
        self.active.lock().await.len()
    }
}
