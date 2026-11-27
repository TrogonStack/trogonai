use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, Port, ProviderCapabilityProfile, SandboxId, TenantId, WorkerId};

use crate::materialized_workspace::MaterializedWorkspace;
use crate::{ModalConfig, ModalProviderError, ModalSandboxApi, VolumeStores, profile};

/// What the worker remembers about one held lease.
pub(crate) struct ActiveModalLease {
    pub(crate) _gate: Arc<Mutex<()>>,
    pub(crate) sandbox_id: SandboxId,
    pub(crate) workspace: Arc<MaterializedWorkspace>,
    pub(crate) tenant: TenantId,
    pub(crate) exposed_ports: Vec<Port>,
}

/// Leases Modal sandboxes, one per lease, and stages each workspace into them.
pub struct ModalProvider {
    pub(crate) api: Arc<dyn ModalSandboxApi>,
    pub(crate) stores: Arc<dyn VolumeStores>,
    pub(crate) config: ModalConfig,
    pub(crate) profile: ProviderCapabilityProfile,
    pub(crate) active: Mutex<HashMap<LeaseId, ActiveModalLease>>,
    gates: std::sync::Mutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
    worker_id: OnceLock<WorkerId>,
}

impl ModalProvider {
    pub fn new(api: Arc<dyn ModalSandboxApi>, stores: Arc<dyn VolumeStores>, config: ModalConfig) -> Self {
        Self {
            api,
            stores,
            profile: profile::for_config(&config),
            config,
            active: Mutex::new(HashMap::new()),
            gates: std::sync::Mutex::new(HashMap::new()),
            worker_id: OnceLock::new(),
        }
    }

    pub fn config(&self) -> &ModalConfig {
        &self.config
    }

    /// The capabilities this worker offers with its current settings.
    pub fn profile(&self) -> &ProviderCapabilityProfile {
        &self.profile
    }

    pub async fn current_leases(&self) -> usize {
        self.active.lock().await.len()
    }

    /// Ties the provider to the worker identity its receipts carry. A second, different worker is refused.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), ModalProviderError> {
        let bound = self.worker_id.get_or_init(|| worker.clone());
        if bound == worker {
            Ok(())
        } else {
            Err(ModalProviderError::WorkerBound(bound.clone()))
        }
    }

    /// Serializes work on one lease. The gate lives as long as someone holds it.
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

    pub(crate) fn bound_worker(&self) -> Option<WorkerId> {
        self.worker_id.get().cloned()
    }
}
