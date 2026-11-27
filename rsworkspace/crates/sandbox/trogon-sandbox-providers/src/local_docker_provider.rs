use std::collections::BTreeMap;
use std::sync::Arc;

use trogon_sandbox_protocol::{LifecycleEmitter, run_phase};

use crate::docker::constants::{DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER, DOCKER_LABEL_ROLE};
use crate::{BollardEngine, ContainerHardening, DockerEngine, DockerError};
use trogon_sandbox_contract::SandboxPhase;

/// Runs sandboxes as containers on the local Docker daemon.
pub struct LocalDockerProvider {
    pub(crate) engine: Arc<dyn DockerEngine>,
    pub(crate) lifecycle: Option<Arc<dyn LifecycleEmitter>>,
    pub(crate) egress_proxy_image: String,
    pub(crate) violation_nats_url: Option<String>,
    pub(crate) hardening: ContainerHardening,
    pub(crate) default_user: Option<String>,
    pub(crate) owner: Option<String>,
}

impl LocalDockerProvider {
    pub fn new(engine: Arc<dyn DockerEngine>, egress_proxy_image: String) -> Self {
        Self {
            engine,
            lifecycle: None,
            egress_proxy_image,
            violation_nats_url: None,
            hardening: ContainerHardening::default(),
            default_user: None,
            owner: None,
        }
    }

    /// Labels every container this provider creates with `owner`, so a restarted
    /// worker with the same owner can find and remove what the last run left behind.
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Labels for a container this provider creates; empty until [`Self::with_owner`] is set.
    pub fn owner_labels(&self, role: &str, lease_id: Option<&str>) -> BTreeMap<String, String> {
        let Some(owner) = self.owner.as_ref() else {
            return BTreeMap::new();
        };
        let mut labels = BTreeMap::from([
            (DOCKER_LABEL_OWNER.to_string(), owner.clone()),
            (DOCKER_LABEL_ROLE.to_string(), role.to_string()),
        ]);
        if let Some(lease_id) = lease_id {
            labels.insert(DOCKER_LABEL_LEASE_ID.to_string(), lease_id.to_string());
        }
        labels
    }

    /// Force-removes every container labelled with this provider's owner, running or
    /// not, and returns how many were removed. Containers with another owner or no
    /// owner label (including those from versions that did not label) are left alone.
    pub async fn sweep_owned(&self) -> Result<usize, DockerError> {
        let Some(owner) = self.owner.as_deref() else {
            return Ok(0);
        };
        let ids = self.engine.list_by_label(DOCKER_LABEL_OWNER, owner).await?;
        for id in &ids {
            self.engine.remove(id).await?;
        }
        Ok(ids.len())
    }

    /// Isolation settings applied to every workload container this provider starts.
    pub fn with_hardening(mut self, hardening: ContainerHardening) -> Self {
        self.hardening = hardening;
        self
    }

    /// User for workloads whose spec does not name one, so images that default
    /// to root can still run unprivileged.
    pub fn with_default_user(mut self, user: Option<String>) -> Self {
        self.default_user = user;
        self
    }

    pub fn hardening(&self) -> &ContainerHardening {
        &self.hardening
    }

    pub fn default_user(&self) -> Option<&str> {
        self.default_user.as_deref()
    }

    /// Enables egress violation events from the sidecar. The sidecar shares
    /// the sandbox container's network namespace, so a loopback host in
    /// `nats_url` is rewritten to `host.docker.internal`; on Linux the Docker
    /// host must resolve that name for the sidecar.
    pub fn with_violation_nats_url(mut self, nats_url: impl Into<String>) -> Self {
        self.violation_nats_url = Some(nats_url.into());
        self
    }

    pub fn with_default_engine(egress_proxy_image: String) -> Result<Self, DockerError> {
        Ok(Self::new(Arc::new(BollardEngine::connect()?), egress_proxy_image))
    }

    pub fn with_lifecycle_emitter(&self, lifecycle: Option<Arc<dyn LifecycleEmitter>>) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            lifecycle,
            egress_proxy_image: self.egress_proxy_image.clone(),
            violation_nats_url: self.violation_nats_url.clone(),
            hardening: self.hardening.clone(),
            default_user: self.default_user.clone(),
            owner: self.owner.clone(),
        }
    }

    pub fn engine(&self) -> Arc<dyn DockerEngine> {
        Arc::clone(&self.engine)
    }

    /// Stops and removes a container started by this provider.
    pub async fn tear_down(
        &self,
        container_id: &str,
        lifecycle: Option<&dyn LifecycleEmitter>,
    ) -> Result<(), DockerError> {
        let engine = Arc::clone(&self.engine);
        let id = container_id.to_string();
        run_phase(
            lifecycle,
            SandboxPhase::Reclaim,
            Some(container_id.to_string()),
            || async move { engine.kill(&id).await },
        )
        .await
    }
}
