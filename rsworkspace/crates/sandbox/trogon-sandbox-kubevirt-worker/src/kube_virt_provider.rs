use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock, Weak};

use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, ProviderCapabilityProfile, WorkerId};

use crate::active_lease::ActiveLease;
use crate::constants::PROFILE_MAX_CONCURRENT_SANDBOXES;
use crate::locked::locked;
use crate::{
    CustodyJournal, ExecGuestProbe, GuestProbe, KubeVirtConfig, KubeVirtOps, KubeVirtProviderError, LeaseResources,
    SnapshotBackend, SshSessionKeys, WarmDiskSource, WarmPoolManager, profile,
};

/// Leases KubeVirt virtual machines, one per lease, and keeps durable custody of everything it creates.
///
/// Every lease writes its create intents and resource receipts to a custody directory before and after it asks the
/// cluster to act, so a restarted worker settles what the last one left behind and never deletes a resource it
/// cannot prove it owns.
pub struct KubeVirtProvider {
    pub(crate) ops: Arc<KubeVirtOps>,
    pub(crate) snapshots: Arc<dyn SnapshotBackend>,
    pub(crate) warm_pool: Arc<WarmPoolManager>,
    pub(crate) journal: Arc<CustodyJournal>,
    pub(crate) guest_probe: Arc<dyn GuestProbe>,
    pub(crate) profile: ProviderCapabilityProfile,
    pub(crate) max_concurrent_leases: u32,
    pub(crate) active: Mutex<HashMap<LeaseId, ActiveLease>>,
    pub(crate) retained: Mutex<HashMap<LeaseId, LeaseResources>>,
    pub(crate) in_flight: std::sync::Mutex<HashMap<LeaseId, Arc<std::sync::Mutex<LeaseResources>>>>,
    pub(crate) preparing: Arc<std::sync::Mutex<HashSet<LeaseId>>>,
    gates: std::sync::Mutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
    pub(crate) ssh_keys: OnceLock<Arc<SshSessionKeys>>,
}

impl KubeVirtProvider {
    /// A provider for `config`'s namespace over `client`, keeping custody in `state_dir`.
    ///
    /// `snapshots` restores and exports disks and also describes the root disk of warm VMs. The directory is locked
    /// for this provider and bound to the worker and namespace of `config`.
    pub fn new(
        config: KubeVirtConfig,
        client: kube::Client,
        state_dir: &Path,
        snapshots: Arc<dyn SnapshotBackend>,
    ) -> Result<Self, KubeVirtProviderError> {
        let journal = CustodyJournal::open(state_dir, config.worker_id.clone(), &config.namespace)?;
        let ops = Arc::new(KubeVirtOps::new(client, config.clone()));
        let source: Arc<dyn WarmDiskSource> = snapshots.clone();
        let warm_pool = Arc::new(WarmPoolManager::new(config.warm_pool, ops.clone(), source));
        Ok(Self {
            ops,
            snapshots,
            warm_pool,
            journal,
            guest_probe: Arc::new(ExecGuestProbe::default()),
            profile: profile(),
            max_concurrent_leases: PROFILE_MAX_CONCURRENT_SANDBOXES,
            active: Mutex::default(),
            retained: Mutex::default(),
            in_flight: std::sync::Mutex::default(),
            preparing: Arc::default(),
            gates: std::sync::Mutex::default(),
            ssh_keys: OnceLock::new(),
        })
    }

    /// Replaces how a new guest is probed once its VMI is ready.
    pub fn with_guest_probe(mut self, probe: Arc<dyn GuestProbe>) -> Self {
        self.guest_probe = probe;
        self
    }

    /// Sets how many leases the worker accepts at once.
    pub fn with_max_concurrent_leases(mut self, max: u32) -> Self {
        self.max_concurrent_leases = max;
        self
    }

    /// Tells the provider which SSH session keys to forget when a lease ends. They are bound after the provider
    /// is built because their guest access reaches leases through the provider.
    pub fn bind_ssh_keys(&self, keys: Arc<SshSessionKeys>) -> Result<(), KubeVirtProviderError> {
        self.ssh_keys.set(keys).map_err(|_| KubeVirtProviderError::SshKeysBound)
    }

    /// The worker's configuration.
    pub fn config(&self) -> &KubeVirtConfig {
        self.ops.config()
    }

    /// The capabilities this worker offers.
    pub fn profile(&self) -> &ProviderCapabilityProfile {
        &self.profile
    }

    /// The worker identity every resource and receipt carries.
    pub fn worker_id(&self) -> &WorkerId {
        &self.ops.config().worker_id
    }

    /// The pool of pre-started VMs snapshot leases claim from.
    pub fn warm_pool(&self) -> &Arc<WarmPoolManager> {
        &self.warm_pool
    }

    /// Serializes work on one lease. The gate lives as long as someone holds it.
    pub(crate) fn authority_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = locked(&self.gates);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease.clone(), Arc::downgrade(&gate));
        gate
    }

    /// What an interrupted or running allocation of `lease` has recorded so far.
    pub(crate) fn in_flight_resources(&self, lease: &LeaseId) -> Option<LeaseResources> {
        let in_flight = locked(&self.in_flight);
        in_flight.get(lease).map(|resources| locked(resources).clone())
    }

    /// Whether any record in memory holds resources for `lease`.
    pub(crate) async fn holds_any(&self, lease: &LeaseId) -> bool {
        self.active.lock().await.contains_key(lease)
            || self.retained.lock().await.contains_key(lease)
            || self.in_flight_resources(lease).is_some()
    }

    /// Whether `lease` is held with exactly the resources `key` names.
    pub(crate) async fn owns(&self, lease: &LeaseId, key: &str) -> bool {
        let active = self
            .active
            .lock()
            .await
            .get(lease)
            .and_then(|active| active.resources.authority_key());
        let retained = self
            .retained
            .lock()
            .await
            .get(lease)
            .and_then(LeaseResources::authority_key);
        let in_flight = self
            .in_flight_resources(lease)
            .and_then(|resources| resources.authority_key());
        [active, retained, in_flight]
            .into_iter()
            .any(|held| held.as_deref() == Some(key))
    }
}
