use std::collections::HashMap;
use std::sync::{Arc, Weak};

use async_nats::jetstream;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, WorkerId};
use trogon_sandbox_cosign::CosignVerifier;
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_providers::{K8sMicroVmProvider, K8sResourceReceipt};

use crate::active_k8s_lease::ActiveK8sLease;
use crate::execution_tracker::ExecutionTracker;
use crate::stored_launch::StoredLaunch;
use crate::{DigestCache, K8sWorkerError};

mod accounts;
mod authority;
mod checkpoint;
mod commands;
mod exec;
mod lease_allocation;
mod network_policy;
mod provider_impl;
mod recovery;
mod rollback;
mod ssh;
mod terminal_backend;

pub struct K8sWorkerProvider {
    pub inner: Arc<K8sMicroVmProvider>,
    pub default_image: String,
    pub js: jetstream::Context,
    pub oci: Arc<OciClient>,
    pub cosign_verifier: Option<CosignVerifier>,
    pub digest_cache: Arc<DigestCache>,
    pub warm_pool: Option<Arc<crate::WarmPool>>,
    pub(crate) active: Arc<Mutex<HashMap<LeaseId, ActiveK8sLease>>>,
    /// Tracks `session_id -> (lease_id, authorized_keys line)` for SSH
    /// sessions attached via [`Self::ssh_attach_for_lease`], so
    /// [`Self::ssh_detach_for_lease`] can strip precisely the right line.
    pub(crate) ssh_sessions: Arc<Mutex<HashMap<String, (LeaseId, String)>>>,
    /// Tracks detached commands started via
    /// [`Self::start_detached_exec_for_lease`], polled via
    /// [`Self::poll_command_execution`].
    pub(crate) executions: ExecutionTracker,
    /// What each detached command was started with, for `restart_command`.
    pub(crate) launches: Arc<Mutex<HashMap<String, StoredLaunch>>>,
    /// Whether each lease's request is also kept in a Secret, so rollback
    /// survives a worker restart. Off by default: the request can carry env
    /// values.
    pub(crate) persist_rollback_state: bool,
    pub(crate) cleanup: Arc<Mutex<HashMap<LeaseId, Vec<K8sResourceReceipt>>>>,
    pub(crate) authority_gates: std::sync::Mutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
}
impl K8sWorkerProvider {
    pub fn new(
        inner: Arc<K8sMicroVmProvider>,
        default_image: String,
        js: jetstream::Context,
        oci: Arc<OciClient>,
        cosign_verifier: Option<CosignVerifier>,
        digest_cache: Arc<DigestCache>,
        warm_pool: Option<Arc<crate::WarmPool>>,
    ) -> Self {
        Self {
            inner,
            default_image,
            js,
            oci,
            cosign_verifier,
            digest_cache,
            warm_pool,
            active: Arc::new(Mutex::new(HashMap::new())),
            ssh_sessions: Arc::new(Mutex::new(HashMap::new())),
            executions: ExecutionTracker::default(),
            launches: Arc::new(Mutex::new(HashMap::new())),
            persist_rollback_state: false,
            cleanup: Default::default(),
            authority_gates: Default::default(),
        }
    }

    /// Keeps each lease's request in a Secret owned by its Job or warm pod, so
    /// an in-place rollback still works after this worker restarts.
    pub fn with_persisted_rollback_state(mut self, persist: bool) -> Self {
        self.persist_rollback_state = persist;
        self
    }

    /// Stores `request` for `lease_id` in the cluster and returns the Secret's
    /// name. A failure is logged and answers `None`: the lease runs, and a
    /// rollback works until the worker restarts.
    async fn persist_request(
        &self,
        inner: &K8sMicroVmProvider,
        owner: &K8sResourceReceipt,
        lease_id: &LeaseId,
        request: &LeaseRequest,
    ) -> Option<K8sResourceReceipt> {
        let bytes = match serde_json::to_vec(request) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%lease_id, %error, "could not encode the lease request, rollback will not survive a restart");
                return None;
            }
        };
        match inner.store_owned_lease_request(owner, lease_id.as_str(), &bytes).await {
            Ok(receipt) => Some(receipt),
            Err(error) => {
                tracing::warn!(%lease_id, %error, "could not store the lease request, rollback will not survive a restart");
                None
            }
        }
    }

    /// The request stored for the Job or warm pod `owner_name`, if the cluster
    /// still holds one that parses.
    async fn load_request(&self, owner_name: &str) -> Option<Arc<LeaseRequest>> {
        let bytes = match self.inner.load_lease_request(owner_name).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(owner = %owner_name, %error, "could not read the stored lease request");
                return None;
            }
        };
        match serde_json::from_slice::<LeaseRequest>(&bytes) {
            Ok(request) => Some(Arc::new(request)),
            Err(error) => {
                tracing::warn!(owner = %owner_name, %error, "the stored lease request does not parse");
                None
            }
        }
    }

    /// The pod that `lease_id`'s commands and exports run in.
    pub async fn sandbox_for_lease(&self, lease_id: &LeaseId) -> Option<String> {
        self.active
            .lock()
            .await
            .get(lease_id)
            .map(|lease| lease.pod_name.clone())
    }

    /// How many leases this worker holds.
    pub async fn active_lease_count(&self) -> usize {
        self.active.lock().await.len()
    }

    /// Whether this worker holds `lease_id`.
    pub async fn holds_lease(&self, lease_id: &LeaseId) -> bool {
        self.sandbox_for_lease(lease_id).await.is_some()
    }

    pub(crate) fn authority_gate(&self, lease_id: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self
            .authority_gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease_id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease_id.clone(), Arc::downgrade(&gate));
        gate
    }

    #[expect(
        clippy::result_large_err,
        reason = "K8sWorkerError carries the provider errors by value so callers can match on them"
    )]
    pub fn worker_id(&self) -> Result<Option<WorkerId>, K8sWorkerError> {
        Ok(self.inner.worker_identity().map(WorkerId::new).transpose()?)
    }
}
