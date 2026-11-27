use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use tokio::sync::{Mutex, watch};
use trogon_sandbox_contract::{LeaseId, NativeSnapshotRef, TenantId, WorkerId};
use trogon_sandbox_protocol::{AttemptLedger, tenant_scoped_kv_key};

use super::{ActiveLease, E2bDispatchError, E2bProviderError};
use crate::api::E2bApi;
use crate::assignments::Assignments;
use crate::exec::E2bExec;
use crate::lease_cleanup::LeaseCleanupReceipts;
use crate::snapshot_custody::SnapshotCustody;
use crate::snapshots::SnapshotManifests;

/// A [`Provider`](trogon_sandbox_protocol::Provider) that leases sandboxes from E2B.
pub struct E2bWorkerProvider {
    pub(crate) api: E2bApi,
    pub(super) default_template: String,
    pub(super) exec: Arc<E2bExec>,
    pub(super) active: Arc<Mutex<HashMap<String, ActiveLease>>>,
    gates: std::sync::Mutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
    pub(super) worker_id: OnceLock<WorkerId>,
    dispatch_ledger: OnceLock<Arc<AttemptLedger>>,
    pub(super) assignments: Assignments,
    ready: watch::Sender<bool>,
    pub(crate) snapshots: SnapshotManifests,
    pub(crate) custody: SnapshotCustody,
    pub(super) cleanup_receipts: LeaseCleanupReceipts,
    snapshot_gates: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl E2bWorkerProvider {
    pub fn new(api: E2bApi, default_template: impl Into<String>, exec: Arc<E2bExec>) -> Self {
        Self {
            api,
            default_template: default_template.into(),
            exec,
            active: Arc::new(Mutex::new(HashMap::new())),
            gates: std::sync::Mutex::new(HashMap::new()),
            worker_id: OnceLock::new(),
            dispatch_ledger: OnceLock::new(),
            assignments: Assignments::default(),
            ready: watch::channel(false).0,
            snapshots: SnapshotManifests::default(),
            custody: SnapshotCustody::default(),
            cleanup_receipts: LeaseCleanupReceipts::default(),
            snapshot_gates: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn exec(&self) -> &Arc<E2bExec> {
        &self.exec
    }

    /// Binds the provider to the worker id it reports in leases. Binding the same id again is fine.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), E2bProviderError> {
        if self.worker_id.get_or_init(|| worker.clone()) != worker {
            return Err(E2bProviderError::BoundToAnotherWorker);
        }
        Ok(())
    }

    pub fn configure_dispatch_ledger(&self, ledger: Arc<AttemptLedger>) -> Result<(), E2bDispatchError> {
        self.bind_worker(ledger.worker_id())?;
        self.dispatch_ledger
            .set(ledger)
            .map_err(|_| E2bDispatchError::LedgerAlreadyConfigured)
    }

    pub fn dispatch_ledger(&self) -> Option<&Arc<AttemptLedger>> {
        self.dispatch_ledger.get()
    }

    pub(crate) fn mark_ready(&self) {
        self.ready.send_replace(true);
    }

    pub async fn wait_ready(&self) {
        let mut ready = self.ready.subscribe();
        let _ = ready.wait_for(|ready| *ready).await;
    }

    pub(super) fn lease_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease.clone(), Arc::downgrade(&gate));
        gate
    }

    pub(crate) fn snapshot_gate(&self, tenant: &TenantId, reference: &NativeSnapshotRef) -> Arc<Mutex<()>> {
        let key = tenant_scoped_kv_key(tenant, reference.id.as_str());
        let mut gates = self
            .snapshot_gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(key, Arc::downgrade(&gate));
        gate
    }
}
