use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    HostMountAccess, IsolationTier, LeaseError, LeaseId, LeaseRequest, NetworkPolicy, PlatformOptions, ProviderKind,
    ProviderLease, SandboxId,
};
use trogon_sandbox_egress_proxy::{ViolationRateLimit, ViolationSink};
use trogon_sandbox_protocol::{
    AuthorityHostError, AuthorityResourceId, AuthorityTaskScope, HeldLease, Provider, ProviderError, PurgeOutcome,
    subjects,
};
use trogon_sandbox_volumes::{
    FuseMetrics, FuseMountConfig, FuseMountHandle, FuseMountManager, FuseOtherUsers, VolumeEndpoint, VolumeRegion,
};

use crate::config::{IsolationBackendKind, WorkerConfig};
use crate::constants::{CANCEL_CONFIRMATION_TIMEOUT, SANDBOX_WORKSPACE_PATH, TERMINATION_POLL};
use crate::fs_violation::{FsDenialSink, FsViolationReporter};
use crate::git_safe_directories;
use crate::isolation::limits::ResourceLimits;
#[cfg(target_os = "linux")]
use crate::isolation::relay::RelayEgress;
use crate::isolation::{IsolationBackend, LaunchedProcess, build_isolation_backend};
pub use crate::launch::{EgressPolicy, ExtraBind, LaunchSpec};
pub use crate::lease_launch::{MaterializedWorkspace, materialize_workspace};
use crate::lease_launch::{agent_env, resolve_agent_binary};
use crate::lease_proxy::{BrokerEndpoint, LeaseProxy, LeaseProxyExtras};
use crate::lease_secrets::{CredentialSources, LeaseSecrets};
use crate::network_policy_needs::NetworkPolicyNeeds;
use crate::proxy_env::ProxyEnvironment;
use crate::tool_proxy_env::{JAVA_TOOL_OPTIONS, ToolProxyEnv};

/// A running lease's process and the switch that makes its watcher kill it.
///
/// The watcher holds the process lock for as long as it waits on the process,
/// so a canceller cannot take that lock to kill it: it signals `stop` instead.
#[derive(Clone)]
struct ActiveLease {
    /// The process id, kept apart from `child` because the watcher holds that
    /// lock for the process's whole life.
    pid: u32,
    authority_id: AuthorityResourceId,
    completed: tokio_util::sync::CancellationToken,
    stop: tokio_util::sync::CancellationToken,
}

pub struct SubprocessWorker {
    authority_gates: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
    authority_tasks: AuthorityTaskScope,
    config: WorkerConfig,
    backend: Arc<dyn IsolationBackend>,
    active: Arc<Mutex<BTreeMap<String, ActiveLease>>>,
    fuse_manager: Option<Arc<FuseMountManager>>,
    credentials: Option<Arc<dyn CredentialSources>>,
    committed_fence: Option<trogon_sandbox_credential_broker::CommittedFence>,
    upstream_ca_bundle: Option<PathBuf>,
    fs_violations: Option<Arc<FsViolationReporter>>,
    tool_proxy_env: ToolProxyEnv,
    workspaces: crate::file_ops::WorkspaceRegistry,
    port_forwards: crate::port_forward::PortForwardRegistry,
    port_bridge_binary: Option<PathBuf>,
    terminals: Option<Arc<crate::terminal::SubprocessTerminals>>,
}

impl SubprocessWorker {
    pub fn new(config: WorkerConfig) -> anyhow::Result<Self> {
        let backend = match config.isolation_backend {
            #[cfg(target_os = "linux")]
            crate::config::IsolationBackendKind::Bubblewrap => {
                let bwrap = config.resolve_bwrap_path()?;
                build_isolation_backend(config.isolation_backend, bwrap)?
            }
            #[cfg(target_os = "macos")]
            crate::config::IsolationBackendKind::Seatbelt => {
                build_isolation_backend(config.isolation_backend, PathBuf::new())?
            }
            crate::config::IsolationBackendKind::None => {
                build_isolation_backend(config.isolation_backend, PathBuf::from("/usr/bin/bwrap"))?
            }
        };

        let fuse_manager = if config.fuse.enabled {
            let region = Some(config.fuse.region.as_str())
                .filter(|region| !region.is_empty())
                .map(VolumeRegion::new)
                .transpose()?;
            let endpoint = config.fuse.endpoint.as_deref().map(VolumeEndpoint::new).transpose()?;
            let fuse_config = FuseMountConfig {
                mount_s3_binary: config.fuse.mount_s3_binary.clone(),
                region,
                endpoint,
                other_users: if config.fuse.allow_other {
                    FuseOtherUsers::Allowed
                } else {
                    FuseOtherUsers::Denied
                },
                ..FuseMountConfig::default()
            };
            Some(Arc::new(FuseMountManager::new(fuse_config, FuseMetrics::new()?)))
        } else {
            None
        };

        Ok(Self {
            authority_gates: std::sync::Mutex::new(HashMap::new()),
            authority_tasks: AuthorityTaskScope::default(),
            config,
            backend,
            active: Arc::new(Mutex::new(BTreeMap::new())),
            fuse_manager,
            credentials: None,
            committed_fence: None,
            upstream_ca_bundle: None,
            fs_violations: None,
            tool_proxy_env: ToolProxyEnv::Disabled,
            workspaces: crate::file_ops::WorkspaceRegistry::new(),
            port_forwards: crate::port_forward::PortForwardRegistry::new(),
            port_bridge_binary: None,
            terminals: None,
        })
    }

    /// How many leases this worker is running now, for the heartbeat. The
    /// count is read without waiting for the lock; a busy lock reports the
    /// last count seen.
    pub fn active_lease_counter(&self) -> Arc<dyn Fn() -> u32 + Send + Sync> {
        let active = Arc::clone(&self.active);
        let last = std::sync::atomic::AtomicU32::new(0);
        Arc::new(move || match active.try_lock() {
            Ok(held) => {
                let count = u32::try_from(held.len()).unwrap_or(u32::MAX);
                last.store(count, std::sync::atomic::Ordering::Relaxed);
                count
            }
            Err(_) => last.load(std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// Shares the registry the worker's `file_op` handler reads, so file operations reach the
    /// workspace of each running lease.
    pub fn with_workspace_registry(mut self, workspaces: crate::file_ops::WorkspaceRegistry) -> Self {
        self.workspaces = workspaces;
        self
    }

    /// Shares the registry the worker's `get_domain` handler reads, so ports
    /// can be forwarded into each running lease.
    pub fn with_port_forwards(mut self, port_forwards: crate::port_forward::PortForwardRegistry) -> Self {
        self.port_forwards = port_forwards;
        self
    }

    /// Bridges forwarded ports through `binary`, which must handle
    /// `--port-bridge` the way the worker binary does. Defaults to the running
    /// executable.
    pub fn with_port_bridge_binary(mut self, binary: PathBuf) -> Self {
        self.port_bridge_binary = Some(binary);
        self
    }

    /// Opens terminals into each running lease through `terminals`.
    pub fn with_terminals(mut self, terminals: Arc<crate::terminal::SubprocessTerminals>) -> Self {
        self.terminals = Some(terminals);
        self
    }

    /// Where real credential values come from, for secret bindings and for
    /// `Transform` rules. Without it a lease that needs either is refused.
    pub fn with_credentials(mut self, credentials: Arc<dyn CredentialSources>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Fences every credential fetch against the lease's committed policy.
    /// With the fence required, a lease that needs credentials but has no
    /// committed policy is refused.
    pub fn with_committed_fence(mut self, fence: trogon_sandbox_credential_broker::CommittedFence) -> Self {
        self.committed_fence = Some(fence);
        self
    }

    /// Roots the lease proxy trusts when it re-encrypts to a real upstream,
    /// in place of the system store.
    pub fn with_upstream_ca_bundle(mut self, bundle: PathBuf) -> Self {
        self.upstream_ca_bundle = Some(bundle);
        self
    }

    /// Publishes the filesystem operations the backend saw refused, through
    /// the same sink the egress proxy publishes its denials on.
    pub fn with_fs_violation_sink(mut self, sink: Arc<dyn ViolationSink>, limit: ViolationRateLimit) -> Self {
        self.fs_violations = Some(Arc::new(FsViolationReporter::new(sink, limit)));
        self
    }

    /// Whether payloads also get the variables for ssh, FTP, Docker and JVMs.
    pub fn with_tool_proxy_env(mut self, tools: ToolProxyEnv) -> Self {
        self.tool_proxy_env = tools;
        self
    }

    pub fn backend(&self) -> Arc<dyn IsolationBackend> {
        Arc::clone(&self.backend)
    }

    pub async fn launch_lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> anyhow::Result<u32> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        anyhow::ensure!(
            !self.active.lock().await.contains_key(lease_id.as_str()),
            "lease assignment is still held or cleaning up"
        );
        ensure_tier_fits_backend(self.config.isolation_backend, req.isolation_tier.as_ref())?;
        ensure_network_policy_fits_backend(self.config.isolation_backend, req.spec.network_policy())?;

        if !req.spec.mounts().is_empty() {
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("FUSE mounts require Linux; this platform is not supported");

            #[cfg(target_os = "linux")]
            if self.fuse_manager.is_none() {
                anyhow::bail!("lease requests mounts but FUSE is disabled; set SANDBOX_FUSE_ENABLED=1");
            }
        }

        if req.spec.privileged().is_granted() {
            anyhow::bail!(
                "privileged workloads require K8sMicroVm or KubeVirt; subprocess/bubblewrap only provides Bwrap-tier isolation"
            );
        }

        if !req.spec.secret_bindings().is_empty() {
            ensure_secrets_fit_backend(self.config.isolation_backend)?;
        }

        ensure_platform_options_fit_backend(self.config.isolation_backend, req.spec.platform())?;

        let binary = resolve_agent_binary(&self.config.agent_binary_default, req.spec.image())?;
        let fence = match &self.committed_fence {
            Some(committed) => committed.resolve(&req.tenant, lease_id, &req.spec).await?,
            None => None,
        };
        let mut secrets = LeaseSecrets::prepare_fenced(
            req.spec.secret_bindings().bindings(),
            &req.tenant,
            self.credentials.as_deref(),
            fence.as_ref(),
        )
        .await?;
        let mut workspace = materialize_workspace(req, &self.config.workspace_root).await?;
        // Held for the life of the lease: dropping it deletes a scratch workspace, which
        // would pull the directory out from under file operations served from the host.
        let scratch = workspace._scratch.take();
        let route = map_egress_route(req.spec.network_policy());
        let (egress_policy, lease_proxy) = match route {
            EgressRoute::Direct(policy) => {
                anyhow::ensure!(
                    !secrets.has_masked_secrets(),
                    "masked secrets need the lease proxy, which this egress policy does not run"
                );
                (policy, None)
            }
            EgressRoute::ThroughProxy(policy) => {
                let broker = if policy.requires_secret_substitution() {
                    let url = self
                        .credentials
                        .as_deref()
                        .and_then(CredentialSources::broker_nats_url)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "the network policy has Transform rules but this worker has no credential broker configured"
                            )
                        })?;
                    Some(BrokerEndpoint {
                        nats_url: url.to_string(),
                        tenant: req.tenant.clone(),
                        fence: fence.clone(),
                    })
                } else {
                    None
                };
                let extras = LeaseProxyExtras {
                    secrets: secrets.take_handoff(),
                    upstream_ca_bundle: self.upstream_ca_bundle.clone(),
                    broker,
                    ca: None,
                };
                let proxy = LeaseProxy::spawn_with(&self.config.resolve_egress_proxy_path()?, policy, extras).await?;
                let egress = self.proxy_egress(&proxy)?;
                (egress, Some(proxy))
            }
        };
        let proxy_environment: Option<ProxyEnvironment> = lease_proxy
            .as_ref()
            .map(|proxy| proxy.environment().with_tools(self.tool_proxy_env.clone()));
        let mut env = build_agent_env(req, lease_id, &workspace.host_path, proxy_environment.as_ref());
        for (key, value) in secrets.env() {
            anyhow::ensure!(
                env.iter().all(|(existing, _)| existing != key),
                "secret binding targets environment variable '{key}', which the lease already sets"
            );
            env.push((key.clone(), value.clone()));
        }

        let (fuse_handles, mut extra_binds) = if req.spec.mounts().is_empty() {
            (vec![], vec![])
        } else {
            self.mount_all(req, lease_id).await?
        };
        extra_binds.extend(proxy_environment.as_ref().and_then(ProxyEnvironment::ca_bind));
        extra_binds.extend(secrets.binds().iter().cloned());

        git_safe_directories::apply(&mut env, &req.spec.platform().git_safe_directories);

        let fs_denials = self.fs_violations.as_ref().map(|reporter| {
            let (sink, denials) = FsDenialSink::channel();
            reporter.drain(lease_id.clone(), denials);
            sink
        });
        let fs_policy = secrets
            .apply_to_fs_policy(req.spec.fs_policy().cloned().filter(|policy| !policy.is_empty()))
            .map_err(|error| anyhow::anyhow!("applying the masked-file rules to the filesystem policy: {error}"))?;
        let spec = LaunchSpec {
            binary,
            args: vec![],
            env,
            workspace_host_path: workspace.host_path.clone(),
            workspace_sandbox_path: PathBuf::from(SANDBOX_WORKSPACE_PATH),
            workspace_readonly: workspace.readonly,
            egress_policy,
            extra_binds,
            user: req.spec.user().map(str::to_owned),
            limits: ResourceLimits::from_spec(req.spec.cpu_seconds().get(), req.spec.memory_mib().get())
                .with_vcpu_millis(req.spec.vcpu_millis()),
            fs_policy,
            platform: req.spec.platform().clone(),
            fs_denials,
        };

        let hold = crate::placeholders::PlaceholderLedger::shared().begin();
        let launch_result = self.backend.launch(&spec).await;

        let LaunchedProcess {
            pid,
            child,
            limits,
            placeholders,
            unguarded,
            monitors,
        } = match launch_result {
            Ok(p) => p,
            Err(e) => {
                drop(hold);
                unmount_all(fuse_handles).await;
                return Err(e);
            }
        };
        hold.record(&placeholders);
        self.workspaces
            .insert(lease_id, workspace.host_path.clone(), workspace.readonly);
        let preview = self
            .port_forwards
            .register_with_preview(lease_id, self.port_reach(&spec, pid))
            .await;
        if let Some(terminals) = &self.terminals {
            // Terminals run in the lease's cgroup so they draw on its limits
            // instead of each carrying a full copy.
            let mut terminal_spec = spec.clone();
            terminal_spec.limits = terminal_spec.limits.sharing_cgroup(limits.cgroup_path());
            terminals.register_lease_with_preview(lease_id, terminal_spec, Arc::clone(&self.backend), preview);
        }

        for path in &unguarded {
            tracing::warn!(
                lease_id = %lease_id,
                path = %path,
                "denied path does not exist and could not be guarded; the sandbox can create it"
            );
        }

        let stop = tokio_util::sync::CancellationToken::new();
        let completed = tokio_util::sync::CancellationToken::new();
        let (authority_id, task_completion) = self
            .authority_tasks
            .track(lease_id.clone(), format!("{pid}-{}", uuid::Uuid::now_v7().simple()))?;
        self.active.lock().await.insert(
            lease_id.as_str().to_owned(),
            ActiveLease {
                pid,
                authority_id: authority_id.clone(),
                completed: completed.clone(),
                stop: stop.clone(),
            },
        );

        let lease_key = lease_id.as_str().to_owned();
        let lease_id_owned = lease_id.clone();
        let active = Arc::clone(&self.active);
        let workspaces = self.workspaces.clone();
        let port_forwards = self.port_forwards.clone();
        let terminals = self.terminals.clone();
        tokio::spawn(async move {
            if let Some(proc) = child.lock().await.as_mut() {
                match lease_proxy {
                    Some(mut proxy) => {
                        tokio::select! {
                            _ = proc.wait() => {}
                            status = proxy.exited() => {
                                tracing::error!(
                                    lease_id = %lease_key,
                                    status = ?status,
                                    "egress proxy exited; terminating the lease"
                                );
                                let _ = proc.kill().await;
                            }
                            _ = stop.cancelled() => {
                                let _ = proc.kill().await;
                            }
                        }
                    }
                    None => {
                        tokio::select! {
                            _ = proc.wait() => {}
                            _ = stop.cancelled() => {
                                let _ = proc.kill().await;
                            }
                        }
                    }
                }
                while !matches!(proc.try_wait(), Ok(Some(_))) {
                    if let Err(error) = proc.kill().await {
                        tracing::warn!(%error, "lease termination remains unconfirmed");
                    }
                    tokio::time::sleep(TERMINATION_POLL).await;
                }
            }
            workspaces.remove(&lease_id_owned);
            port_forwards.remove(&lease_id_owned).await;
            if let Some(terminals) = terminals {
                terminals.forget_lease(&lease_id_owned).await;
            }
            drop(scratch);
            limits.release().await;
            hold.release();
            unmount_all(fuse_handles).await;
            drop(secrets);
            drop(monitors);
            let mut active = active.lock().await;
            if active
                .get(&lease_key)
                .is_some_and(|lease| lease.authority_id == authority_id)
            {
                active.remove(&lease_key);
            }
            task_completion.confirm();
            completed.cancel();
        });

        Ok(pid)
    }

    /// How a port of this lease can be reached from the host: only a
    /// bubblewrap lease in a network namespace of its own has a loopback the
    /// worker can enter. Anywhere else a port the lease opens is a host port.
    fn port_reach(&self, spec: &LaunchSpec, pid: u32) -> crate::port_forward::Reach {
        #[cfg(target_os = "linux")]
        if matches!(
            self.config.isolation_backend,
            crate::config::IsolationBackendKind::Bubblewrap
        ) && !crate::isolation::share_network(&spec.egress_policy)
        {
            return crate::port_forward::Reach::Process {
                pid,
                helper: self.port_bridge_binary.clone(),
            };
        }
        let _ = (spec, pid);
        crate::port_forward::Reach::Unsupported
    }

    #[cfg(target_os = "linux")]
    fn proxy_egress(&self, proxy: &LeaseProxy) -> anyhow::Result<EgressPolicy> {
        let relay = match &self.config.relay_binary_path {
            Some(relay_binary) => RelayEgress {
                proxy_socket: proxy.socket().to_path_buf(),
                relay_binary: relay_binary.clone(),
            },
            None => proxy.relay_egress()?,
        };
        Ok(EgressPolicy::Relay(relay))
    }

    #[cfg(target_os = "macos")]
    fn proxy_egress(&self, proxy: &LeaseProxy) -> anyhow::Result<EgressPolicy> {
        Ok(EgressPolicy::Loopback(proxy.loopback_addr()))
    }

    async fn mount_all(
        &self,
        req: &LeaseRequest,
        lease_id: &LeaseId,
    ) -> anyhow::Result<(Vec<FuseMountHandle>, Vec<ExtraBind>)> {
        let manager = self
            .fuse_manager
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("lease requests mounts but FUSE is disabled; set SANDBOX_FUSE_ENABLED=1"))?;

        let fuse_base = self.config.workspace_root.join(".fuse-mounts").join(lease_id.as_str());

        let mut handles: Vec<FuseMountHandle> = Vec::with_capacity(req.spec.mounts().len());
        let mut binds: Vec<ExtraBind> = Vec::with_capacity(req.spec.mounts().len());

        for (idx, mount) in req.spec.mounts().iter().enumerate() {
            let host_dir = fuse_base.join(idx.to_string());
            match manager
                .mount(
                    &mount.source,
                    &host_dir,
                    HostMountAccess::from(mount.access.is_read_only()),
                )
                .await
            {
                Ok(handle) => {
                    binds.push(ExtraBind {
                        host_path: host_dir,
                        sandbox_path: mount.mount_path.to_string(),
                        readonly: mount.access.is_read_only(),
                        credential_mask: false,
                    });
                    handles.push(handle);
                }
                Err(e) => {
                    unmount_all(handles).await;
                    return Err(anyhow::anyhow!("failed to mount {}: {e}", mount.mount_path));
                }
            }
        }

        Ok((handles, binds))
    }

    /// The ids of the leases this worker is running.
    pub async fn held_lease_ids(&self) -> Vec<LeaseId> {
        self.active
            .lock()
            .await
            .keys()
            .map(|id| LeaseId::new(id.as_str()))
            .collect()
    }

    /// The process id of a running lease.
    pub async fn lease_pid(&self, lease_id: &LeaseId) -> Option<u32> {
        self.active.lock().await.get(lease_id.as_str()).map(|lease| lease.pid)
    }

    fn authority_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self
            .authority_gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(lease.as_str()).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(lease.as_str().to_owned(), Arc::downgrade(&gate));
        gate
    }

    pub async fn authority_held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self
            .active
            .lock()
            .await
            .iter()
            .map(|(id, lease)| HeldLease::Provider {
                lease_id: LeaseId::new(id.as_str()),
                provider: ProviderKind::Subprocess,
                resource_id: lease.authority_id.clone(),
            })
            .collect())
    }

    pub async fn authority_purge(&self, expected: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Subprocess,
            resource_id,
        } = expected
        else {
            return Ok(PurgeOutcome::Stale);
        };
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let record = self
            .active
            .lock()
            .await
            .get(lease_id.as_str())
            .filter(|lease| &lease.authority_id == resource_id)
            .cloned();
        let Some(record) = record else {
            return Ok(if self.authority_tasks.is_completed(lease_id, resource_id) {
                PurgeOutcome::Settled
            } else {
                PurgeOutcome::Stale
            });
        };
        record.stop.cancel();
        tokio::time::timeout(CANCEL_CONFIRMATION_TIMEOUT, record.completed.cancelled())
            .await
            .map_err(|_| AuthorityHostError::CleanupFailed {
                message: "exact subprocess task termination remains unconfirmed".to_owned(),
            })?;
        Ok(PurgeOutcome::Purged)
    }

    pub async fn cancel(&self, lease_id: &LeaseId) -> anyhow::Result<()> {
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        let record = self.active.lock().await.get(lease_id.as_str()).cloned();
        if let Some(record) = record {
            record.stop.cancel();
            tokio::time::timeout(CANCEL_CONFIRMATION_TIMEOUT, record.completed.cancelled())
                .await
                .map_err(|_| anyhow::anyhow!("subprocess task termination remains unconfirmed"))?;
        }
        Ok(())
    }
}

/// The backend `none` isolates nothing, so a lease that asks for an isolation
/// tier cannot be honoured by it.
pub fn ensure_tier_fits_backend(backend: IsolationBackendKind, tier: Option<&IsolationTier>) -> anyhow::Result<()> {
    if matches!(backend, crate::config::IsolationBackendKind::None) && tier.is_some() {
        anyhow::bail!("isolation backend `none` is only allowed when isolation_tier is unset");
    }
    Ok(())
}

/// The backend `none` has no network namespace, so it cannot hold a lease to
/// `DenyAll` or to an allowlist: the payload would run with the host's network.
/// A lease that names such a policy is refused with a typed error rather than
/// served with open egress. A lease that names no policy, and `AllowAll`, ask
/// for nothing the backend withholds.
pub fn ensure_network_policy_fits_backend(
    backend: IsolationBackendKind,
    policy: Option<&NetworkPolicy>,
) -> anyhow::Result<()> {
    if matches!(backend, crate::config::IsolationBackendKind::None)
        && matches!(policy, Some(NetworkPolicy::DenyAll | NetworkPolicy::UserDefined { .. }))
    {
        return Err(anyhow::Error::new(
            LeaseError::InvalidNetworkPolicy {
                reason: "isolation backend `none` cannot restrict the network; use the bubblewrap or seatbelt backend, or AllowAll".to_string(),
            },
        ));
    }
    Ok(())
}

/// The backend `none` cannot keep a secret from the payload.
pub fn ensure_secrets_fit_backend(backend: crate::config::IsolationBackendKind) -> anyhow::Result<()> {
    anyhow::ensure!(
        !matches!(backend, crate::config::IsolationBackendKind::None),
        "secret bindings need the bubblewrap isolation backend, which `none` is not"
    );
    Ok(())
}

pub fn ensure_platform_options_fit_backend(
    backend: IsolationBackendKind,
    options: &PlatformOptions,
) -> anyhow::Result<()> {
    let isolated = !matches!(backend, IsolationBackendKind::None);
    crate::isolation::ensure_platform_options_enforceable(
        options,
        isolated && cfg!(target_os = "macos"),
        isolated && cfg!(target_os = "linux"),
    )
}

async fn unmount_all(handles: Vec<FuseMountHandle>) {
    for handle in handles {
        let _ = handle.unmount().await;
    }
}

enum EgressRoute<'a> {
    Direct(EgressPolicy),
    ThroughProxy(&'a NetworkPolicy),
}

fn map_egress_route(policy: Option<&NetworkPolicy>) -> EgressRoute<'_> {
    match policy {
        None | Some(NetworkPolicy::DenyAll) => EgressRoute::Direct(EgressPolicy::DenyAll),
        Some(NetworkPolicy::AllowAll) => EgressRoute::Direct(EgressPolicy::AllowAll),
        Some(policy @ NetworkPolicy::UserDefined { .. }) => EgressRoute::ThroughProxy(policy),
    }
}

fn build_agent_env(
    req: &LeaseRequest,
    lease_id: &LeaseId,
    workspace: &Path,
    proxy: Option<&ProxyEnvironment>,
) -> Vec<(String, String)> {
    let mut env = agent_env(req, lease_id, workspace);
    if let Some(proxy) = proxy {
        let inherited = env
            .iter()
            .position(|(name, _)| name == JAVA_TOOL_OPTIONS)
            .map(|at| env.remove(at).1);
        env.extend(proxy.vars());
        if let Some(options) = proxy.java_tool_options(inherited.as_deref()) {
            env.push((JAVA_TOOL_OPTIONS.to_string(), options));
        }
    }
    env
}

#[async_trait]
impl Provider for SubprocessWorker {
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
        SubprocessWorker::cancel(self, lease_id)
            .await
            .map_err(ProviderError::backend)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
