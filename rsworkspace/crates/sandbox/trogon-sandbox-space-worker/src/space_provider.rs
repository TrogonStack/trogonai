use std::fs::File;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, LeaseRequest, NetworkPolicy, ProviderCapabilityProfile, ProviderKind,
    WorkspaceSource,
};
use trogon_sandbox_protocol::AttemptLedger;

use crate::profile::capability_profile;
use crate::{
    CreateBody, CreateMetadata, CreateResources, LeaseViolationError, Registry, SpaceApi, SpaceConfig,
    SpaceProviderOpenError,
};

/// A provider that leases sandboxes as Firecracker microVMs through the SPACE native API.
///
/// One provider owns one state directory. The directory holds an exclusive lock, so a
/// second worker cannot run against the same ledger, and the dispatch ledger that lets a
/// restart find and delete what a crashed run left behind.
pub struct SpaceProvider {
    pub(crate) api: SpaceApi,
    pub(crate) config: SpaceConfig,
    pub(crate) owner: String,
    pub(crate) registry: Mutex<Registry>,
    pub(crate) ledger: Arc<AttemptLedger>,
    _state_lock: File,
}

#[cfg(unix)]
fn restrict(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::set_permissions(
        dir,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )
}

#[cfg(not(unix))]
fn restrict(_dir: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

impl SpaceProvider {
    /// Opens the state directory, takes its lock and opens the dispatch ledger.
    pub fn new(api: SpaceApi, config: SpaceConfig) -> Result<Self, SpaceProviderOpenError> {
        std::fs::create_dir_all(&config.state_dir)?;
        restrict(&config.state_dir)?;
        let state_lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.state_dir.join("worker.lock"))?;
        state_lock.try_lock()?;
        let database = config.state_dir.join("dispatch.db");
        let ledger = Arc::new(AttemptLedger::open(database, ProviderKind::Space, None)?);
        Ok(Self {
            owner: ledger.worker_id().as_str().to_owned(),
            api,
            config,
            ledger,
            registry: Mutex::new(Registry::default()),
            _state_lock: state_lock,
        })
    }

    /// What this worker offers, with its configured capacity.
    pub fn profile(&self) -> ProviderCapabilityProfile {
        capability_profile(self.config.max_leases.get())
    }

    /// The durable record of dispatch attempts.
    pub fn dispatch_ledger(&self) -> &Arc<AttemptLedger> {
        &self.ledger
    }

    /// Leases held or being created.
    pub async fn current_leases(&self) -> usize {
        let registry = self.registry.lock().await;
        registry.active.len() + registry.pending.len()
    }

    /// Whether this worker holds the lease.
    pub async fn holds_lease(&self, id: &LeaseId) -> bool {
        self.registry.lock().await.active.contains_key(id.as_str())
    }

    pub(crate) fn validate_lease(&self, request: &LeaseRequest) -> Result<(), LeaseViolationError> {
        let spec = &request.spec;
        if !self.config.template.serves(spec.image()) {
            return Err(LeaseViolationError::Template);
        }
        if spec.privileged().is_granted() {
            return Err(LeaseViolationError::Privileged);
        }
        if !(matches!(spec.workspace(), WorkspaceSource::Empty(_))
            && spec.mounts().is_empty()
            && spec.drives().is_empty())
        {
            return Err(LeaseViolationError::Workspace);
        }
        if !spec.exposed_ports().is_empty() {
            return Err(LeaseViolationError::Ports);
        }
        if !matches!(spec.network_policy(), None | Some(NetworkPolicy::AllowAll)) {
            return Err(LeaseViolationError::Egress);
        }
        if !(spec.fs_policy().is_none_or(|policy| policy.is_empty())
            && spec.secret_bindings().is_empty()
            && spec.platform().is_default()
            && spec.user().is_none())
        {
            return Err(LeaseViolationError::Policies);
        }
        if request.ttl_seconds == 0 || request.persistent.is_persistent() || request.prewarm.is_prewarm() {
            return Err(LeaseViolationError::Modes);
        }
        match spec.working_dir() {
            Some(dir) if !dir.starts_with('/') || dir.contains('\0') => Err(LeaseViolationError::WorkingDirectory),
            _ => Ok(()),
        }
    }

    pub(crate) fn create_body(
        &self,
        request: &LeaseRequest,
        lease: &LeaseId,
        attempt: &str,
        identity: Option<&LeaseAttemptIdentity>,
    ) -> CreateBody {
        let spec = &request.spec;
        CreateBody {
            template: self.config.template.as_str().to_owned(),
            runtime: "fc",
            env: spec.env().clone(),
            metadata: CreateMetadata {
                sandbox_owner: self.owner.clone(),
                sandbox_lease: lease.as_str().to_owned(),
                sandbox_attempt: attempt.to_owned(),
                sandbox_dispatch_attempt: identity.map(|identity| identity.target.attempt_id.as_str().to_owned()),
            },
            resources: CreateResources {
                vcpu: spec.vcpu_millis().map(|millis| millis.div_ceil(1000).max(1)),
                memory_mib: spec.memory_mib().get(),
                disk_mib: spec.disk_mib(),
            },
        }
    }
}
