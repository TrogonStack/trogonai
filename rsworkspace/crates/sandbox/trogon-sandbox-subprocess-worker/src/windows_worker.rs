//! Serves leases on Windows. Every lease runs in its own AppContainer with the
//! network denied, unless the network fence is installed and the worker
//! has an egress proxy: then a lease with a domain allowlist runs under the
//! fence's sandbox account, fenced to a lease proxy on loopback, and may ask
//! for TLS interception with the CA the sandbox account trusts. A lease that
//! needs secret bindings is refused before anything starts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Context as _;
use async_trait::async_trait;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, NetworkPolicy, ProviderCapabilityProfile, ProviderKind, ProviderLease, SandboxId,
};
use trogon_sandbox_protocol::{Provider, ProviderError, subjects};
use trogon_sandbox_windows_fence::{
    FenceHelper, FsAccessSet, InstallContext, ProxyPortRange, SandboxStatus, WindowsFence, check_capabilities,
    check_dependencies,
};

use crate::admission::{Egress, admit_with};
use crate::constants::{FENCED_PROFILE, PROFILE};
use crate::isolation::appcontainer::{AppContainerBackend, AppContainerProcess};
use crate::isolation::fenced::{FencedBackend, FencedProcess};
use crate::isolation::resource_limits::ResourceLimits;
use crate::launch::{EgressPolicy, LaunchSpec};
use crate::lease_launch::{agent_env, materialize_workspace, resolve_agent_binary};
use crate::lease_proxy::{CaFilePaths, LeaseProxy, LeaseProxyExtras};
use crate::network_policy_needs::NetworkPolicyNeeds;
use crate::windows_ca::{TrustBundle, persistent_ca_files};

#[derive(Debug, Clone)]
pub struct WindowsWorkerConfig {
    pub agent_binary_default: PathBuf,
    pub agent_args: Vec<String>,
    pub workspace_root: PathBuf,
    /// The egress proxy binary a fenced lease routes through. Without it the
    /// network stays denied for every lease.
    pub egress_proxy_binary: Option<PathBuf>,
    /// The fence helper executable that drives the fence; the worker's own
    /// executable when unset, which answers to `--fence-helper`.
    pub fence_helper: Option<PathBuf>,
}

enum LeaseProcess {
    AppContainer(AppContainerProcess),
    Fenced(FencedProcess),
}

impl LeaseProcess {
    async fn wait(&self) -> anyhow::Result<()> {
        match self {
            Self::AppContainer(process) => process.wait().await.map(drop),
            Self::Fenced(process) => process.wait().await.map(drop),
        }
    }

    fn kill(&self) {
        match self {
            Self::AppContainer(process) => process.kill(),
            Self::Fenced(process) => process.kill(),
        }
    }
}

type ActiveLeases = Arc<Mutex<HashMap<String, Arc<LeaseProcess>>>>;

fn locked(active: &ActiveLeases) -> MutexGuard<'_, HashMap<String, Arc<LeaseProcess>>> {
    active.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The fence a worker found installed, and the proxy a lease is fenced to.
struct Fence {
    backend: FencedBackend,
    ports: ProxyPortRange,
    egress_proxy_binary: PathBuf,
    api: WindowsFence,
    status: SandboxStatus,
    ca: tokio::sync::OnceCell<Arc<WorkerCa>>,
}

impl Fence {
    /// The persistent CA, ensured and trusted by the sandbox account on the
    /// first lease that terminates TLS, and kept for the life of the worker.
    async fn ca(&self) -> anyhow::Result<Arc<WorkerCa>> {
        self.ca
            .get_or_try_init(|| async {
                let api = self.api.clone();
                let status = self.status.clone();
                let ca = tokio::task::spawn_blocking(move || WorkerCa::provision(api, &status))
                    .await
                    .context("provisioning the CA")??;
                Ok(Arc::new(ca))
            })
            .await
            .cloned()
    }
}

/// The CA a lease proxy signs with and the bundle a lease trusts. The sandbox
/// account is granted read on the bundle directory for as long as this lives.
struct WorkerCa {
    files: CaFilePaths,
    bundle: TrustBundle,
    release: Option<(WindowsFence, String)>,
}

impl WorkerCa {
    fn provision(api: WindowsFence, status: &SandboxStatus) -> anyhow::Result<Self> {
        let sid = status.user.sid.clone().context("the sandbox account has no SID")?;
        let files = persistent_ca_files(&api, status)?;
        let bundle = TrustBundle::write(&files.cert, &[])?;
        let read = FsAccessSet {
            grant_read: vec![bundle.dir().to_string_lossy().into_owned()],
            ..FsAccessSet::default()
        };
        api.acl_grant_persistent(&sid, std::process::id(), &read, &[])?;
        Ok(Self {
            files,
            bundle,
            release: Some((api, sid)),
        })
    }
}

impl Drop for WorkerCa {
    fn drop(&mut self) {
        if let Some((api, sid)) = self.release.take()
            && let Err(reason) = api.acl_release(&sid, std::process::id())
        {
            tracing::warn!(%reason, "the read grant on the CA bundle was not fully released");
        }
    }
}

pub struct WindowsWorker {
    config: WindowsWorkerConfig,
    backend: AppContainerBackend,
    fence: Option<Fence>,
    active: ActiveLeases,
    workspaces: crate::file_ops::WorkspaceRegistry,
}

impl WindowsWorker {
    pub fn new(config: WindowsWorkerConfig) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.workspace_root)
            .with_context(|| format!("creating the workspace root {}", config.workspace_root.display()))?;
        let backend = AppContainerBackend::new(&config.workspace_root)?;
        let fence = find_fence(&config);
        Ok(Self {
            config,
            backend,
            fence,
            active: Arc::default(),
            workspaces: crate::file_ops::WorkspaceRegistry::new(),
        })
    }

    /// Shares the registry the file_op handler reads, so it can serve the
    /// workspace of each running lease.
    pub fn with_workspace_registry(mut self, workspaces: crate::file_ops::WorkspaceRegistry) -> Self {
        self.workspaces = workspaces;
        self
    }

    /// The capabilities this worker advertises: a domain allowlist only when
    /// the fence it enforces with is installed.
    pub fn profile(&self) -> &'static ProviderCapabilityProfile {
        if self.fence.is_some() {
            &FENCED_PROFILE
        } else {
            &PROFILE
        }
    }

    /// Deletes the AppContainer profiles a crashed worker left behind. Run it
    /// once at startup, before the first lease.
    pub fn sweep_leftover_profiles(&self) -> usize {
        self.backend.sweep_leftover_profiles()
    }

    pub fn active_leases(&self) -> u32 {
        u32::try_from(locked(&self.active).len()).unwrap_or(u32::MAX)
    }

    pub fn active_lease_counter(&self) -> Arc<dyn Fn() -> u32 + Send + Sync> {
        let active = Arc::clone(&self.active);
        Arc::new(move || u32::try_from(locked(&active).len()).unwrap_or(u32::MAX))
    }

    pub async fn launch_lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> anyhow::Result<u32> {
        let egress = if self.fence.is_some() {
            Egress::DomainAllowlistTerminatingTls
        } else {
            Egress::Denied
        };
        admit_with(req, self.profile(), egress)?;

        let binary = resolve_agent_binary(&self.config.agent_binary_default, req.spec.image())?;
        let workspace = materialize_workspace(req, &self.config.workspace_root).await?;
        let env = agent_env(req, lease_id, &workspace.host_path);
        let spec = LaunchSpec {
            binary,
            args: self.config.agent_args.clone(),
            env,
            workspace_host_path: workspace.host_path.clone(),
            workspace_sandbox_path: workspace.host_path.clone(),
            workspace_readonly: workspace.readonly,
            egress_policy: EgressPolicy::DenyAll,
            extra_binds: vec![],
            user: None,
            limits: ResourceLimits::from_spec(req.spec.cpu_seconds().get(), req.spec.memory_mib().get())
                .with_vcpu_millis(req.spec.vcpu_millis()),
            fs_policy: req.spec.fs_policy().cloned().filter(|policy| !policy.is_empty()),
            platform: Default::default(),
            fs_denials: None,
        };

        let (process, proxy) = match (&self.fence, req.spec.network_policy()) {
            (Some(fence), Some(policy @ NetworkPolicy::UserDefined { .. })) => {
                let (process, proxy) = launch_fenced(fence, spec, policy).await?;
                (LeaseProcess::Fenced(process), Some(proxy))
            }
            (Some(fence), _) if spec.fs_policy.is_some() => {
                let backend = fence.backend.clone();
                let process = tokio::task::spawn_blocking(move || backend.launch(&spec, vec![]))
                    .await
                    .context("launching the lease")??;
                (LeaseProcess::Fenced(process), None)
            }
            _ => {
                let backend = self.backend.clone();
                let process = tokio::task::spawn_blocking(move || backend.launch(&spec))
                    .await
                    .context("launching the lease")??;
                (LeaseProcess::AppContainer(process), None)
            }
        };
        let pid = match &process {
            LeaseProcess::AppContainer(process) => process.pid(),
            LeaseProcess::Fenced(process) => process.pid(),
        };
        let process = Arc::new(process);
        locked(&self.active).insert(lease_id.to_string(), Arc::clone(&process));
        self.workspaces
            .insert(lease_id, workspace.host_path.clone(), workspace.readonly);

        let lease_key = lease_id.to_string();
        let lease_id_owned = lease_id.clone();
        let active = Arc::clone(&self.active);
        let workspaces = self.workspaces.clone();
        tokio::spawn(async move {
            if let Err(error) = process.wait().await {
                tracing::warn!(lease_id = %lease_key, %error, "waiting for the lease failed");
            }
            locked(&active).remove(&lease_key);
            workspaces.remove(&lease_id_owned);
            drop(process);
            drop(proxy);
            drop(workspace);
        });

        Ok(pid)
    }

    pub fn cancel(&self, lease_id: &LeaseId) {
        self.workspaces.remove(lease_id);
        let process = locked(&self.active).remove(lease_id.as_str());
        if let Some(process) = process {
            process.kill();
        }
    }
}

/// Looks for an installed fence the lease proxy can be fenced to. Anything
/// short of a usable one leaves the network denied for every lease.
fn find_fence(config: &WindowsWorkerConfig) -> Option<Fence> {
    let egress_proxy_binary = config.egress_proxy_binary.clone()?;
    let found = (|| -> anyhow::Result<Fence> {
        let exe = match &config.fence_helper {
            Some(path) => path.clone(),
            None => std::env::current_exe().context("locating the worker executable")?,
        };
        let fence_helper = FenceHelper::new(exe)?;
        let api = WindowsFence::new(fence_helper.clone());
        let status = api.status(None)?;
        if let Some(error) = check_dependencies(&status.user, &status.wfp, &InstallContext::default()).error() {
            return Err(error.into());
        }
        check_capabilities(&status.user)?;
        let ports = status.lease_proxy_ports()?;
        Ok(Fence {
            backend: FencedBackend::new(fence_helper, ports),
            ports,
            egress_proxy_binary,
            api,
            status,
            ca: tokio::sync::OnceCell::new(),
        })
    })();
    match found {
        Ok(fence) => Some(fence),
        Err(error) => {
            tracing::warn!(%error, "the network fence is not usable; every lease keeps the network denied");
            None
        }
    }
}

async fn launch_fenced(
    fence: &Fence,
    mut spec: LaunchSpec,
    policy: &NetworkPolicy,
) -> anyhow::Result<(FencedProcess, LeaseProxy)> {
    let ca = if policy.requires_tls_intercept() {
        Some(fence.ca().await?)
    } else {
        None
    };
    let proxy = LeaseProxy::spawn_fenced(
        &fence.egress_proxy_binary,
        policy,
        fence.ports,
        LeaseProxyExtras {
            ca: ca.as_ref().map(|ca| ca.files.clone()),
            ..LeaseProxyExtras::default()
        },
    )
    .await?;
    spec.egress_policy = EgressPolicy::Loopback(proxy.loopback_addr());
    let vars = match &ca {
        Some(ca) => proxy.environment_trusting(ca.bundle.path().to_path_buf()),
        None => proxy.environment(),
    }
    .vars();
    let backend = fence.backend.clone();
    let process = tokio::task::spawn_blocking(move || backend.launch(&spec, vars))
        .await
        .context("launching the lease")??;
    Ok((process, proxy))
}

#[async_trait]
impl Provider for WindowsWorker {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Subprocess
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let pid = self.launch_lease(req, lease_id).await.map_err(ProviderError::backend)?;
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(pid.to_string()).map_err(ProviderError::backend)?,
            evidence_subject: subjects::evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
        })
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        WindowsWorker::cancel(self, lease_id);
        Ok(())
    }
}
