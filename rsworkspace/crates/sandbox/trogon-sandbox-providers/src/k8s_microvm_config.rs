use std::collections::BTreeMap;

use trogon_sandbox_contract::IsolationTier;

use crate::{K8sResourceNaming, LABEL_WORKER_IDENTITY, ORPHAN_RECLAIM_GRACE_SECONDS};

#[derive(Clone)]
pub struct K8sMicroVmConfig {
    pub resource_naming: K8sResourceNaming,
    pub namespace: String,
    pub runtime_class: String,
    pub runtime_class_by_tier: BTreeMap<IsolationTier, String>,
    pub service_account: String,
    pub image_pull_secrets: Vec<String>,
    /// Stable identity of this worker replica (for example its StatefulSet
    /// pod name). When set, lease resources carry it in
    /// [`LABEL_WORKER_IDENTITY`] and recovery skips resources another replica
    /// owns. `None` keeps the single-replica behaviour of recovering every
    /// lease in the namespace.
    pub worker_identity: Option<String>,
    /// Whether a replica with a `worker_identity` still adopts lease
    /// resources that carry no identity label, such as ones launched before
    /// the label existed. Two replicas both adopting the same unlabeled lease
    /// is the collision the label prevents, so turn this off once no
    /// unlabeled leases remain.
    pub recover_unlabeled_leases: bool,
    /// Whether an agent whose lease names no user keeps the image's own
    /// non-root `USER` instead of being forced to [`DEFAULT_AGENT_UID`](crate::DEFAULT_AGENT_UID). An
    /// image whose `USER` is root, unset or a name rather than a numeric id
    /// falls back to [`DEFAULT_AGENT_UID`](crate::DEFAULT_AGENT_UID) with a warning, so the container
    /// never runs as root (`runAsNonRoot` stays on). Needs an
    /// [`ImageUserResolver`](crate::ImageUserResolver) on the provider; without one every image falls
    /// back. Off by default, which forces the uid for every image.
    pub honor_image_user: bool,
    /// Seconds a no-deadline lease resource owned by a worker that no longer
    /// exists must age before this replica reclaims it; zero turns the reclaim
    /// off. See [`ORPHAN_RECLAIM_GRACE_SECONDS`].
    pub orphan_reclaim_grace_seconds: i64,
}

impl K8sMicroVmConfig {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            resource_naming: K8sResourceNaming::Sandbox,
            runtime_class: "kata-fc".to_string(),
            runtime_class_by_tier: BTreeMap::new(),
            service_account: "sandbox-agent".to_string(),
            image_pull_secrets: vec![],
            worker_identity: None,
            recover_unlabeled_leases: true,
            honor_image_user: false,
            orphan_reclaim_grace_seconds: ORPHAN_RECLAIM_GRACE_SECONDS,
        }
    }

    pub fn with_orphan_reclaim_grace_seconds(mut self, seconds: i64) -> Self {
        self.orphan_reclaim_grace_seconds = seconds;
        self
    }

    pub fn with_honor_image_user(mut self, honor: bool) -> Self {
        self.honor_image_user = honor;
        self
    }

    pub fn with_worker_identity(mut self, identity: impl Into<String>) -> Self {
        self.worker_identity = Some(identity.into());
        self
    }

    pub fn with_recover_unlabeled_leases(mut self, recover: bool) -> Self {
        self.recover_unlabeled_leases = recover;
        self
    }

    /// Whether this replica owns the lease resource carrying `labels`.
    pub fn owns_lease_resource(&self, labels: &BTreeMap<String, String>) -> bool {
        match (&self.worker_identity, labels.get(LABEL_WORKER_IDENTITY)) {
            (None, _) => true,
            (Some(me), Some(owner)) => me == owner,
            (Some(_), None) => self.recover_unlabeled_leases,
        }
    }

    pub fn with_runtime_class(mut self, rc: impl Into<String>) -> Self {
        self.runtime_class = rc.into();
        self
    }

    pub fn with_runtime_class_for_tier(mut self, tier: IsolationTier, rc: impl Into<String>) -> Self {
        self.runtime_class_by_tier.insert(tier, rc.into());
        self
    }

    pub fn with_resource_naming(mut self, naming: K8sResourceNaming) -> Self {
        self.resource_naming = naming;
        self
    }

    pub fn with_service_account(mut self, sa: impl Into<String>) -> Self {
        self.service_account = sa.into();
        self
    }

    pub fn with_image_pull_secrets(mut self, secrets: Vec<String>) -> Self {
        self.image_pull_secrets = secrets;
        self
    }

    /// Runtime class to use for a lease. Picks the tier-specific entry when
    /// available, else the configured default.
    pub fn runtime_class_for(&self, tier: Option<IsolationTier>) -> &str {
        tier.and_then(|t| self.runtime_class_by_tier.get(&t))
            .map(String::as_str)
            .unwrap_or(self.runtime_class.as_str())
    }
}
