use super::*;

pub struct DockerWorkerProvider {
    pub(super) dispatch_ledger: Option<Arc<trogon_sandbox_protocol::AttemptLedger>>,
    pub(super) intake_ready: AtomicBool,
    pub(super) intake_notify: tokio::sync::Notify,
    pub(super) command_runtime: Option<Arc<command_runtime::CommandRuntime>>,
    pub(super) command_custody: command_custody::Custody,
    pub(super) controlled_executions: Mutex<HashMap<ExecutionId, controlled::ControlledExecution>>,
    pub(super) authority_gates: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
    pub(super) authority_paused: Mutex<HashSet<trogon_sandbox_protocol::HeldLease>>,
    pub inner: LocalDockerProvider,
    pub default_image: String,
    pub(super) browser_support: Option<crate::browser::BrowserSupport>,
    pub js: jetstream::Context,
    pub nats_url: String,
    pub worker_id: String,
    pub oci_client: OciClient,
    pub default_registry: String,
    pub cosign_verifier: Option<CosignVerifier>,
    pub digest_cache: Arc<DigestCache>,
    pub volume_region: Option<VolumeRegion>,
    pub volume_endpoint: Option<VolumeEndpoint>,
    pub(super) warm_pool: Arc<Mutex<WarmPool>>,
    pub(super) active: Arc<Mutex<HashMap<String, ActiveDockerLease>>>,
    pub(super) native_custody: Mutex<native_birth::Registry>,
    pub(super) fuse_manager: Arc<FuseMountManager>,
    pub(super) fuse_handles: Arc<Mutex<HashMap<String, Vec<FuseMountHandle>>>>,
    pub(crate) warm_grab_count: Arc<AtomicU32>,
    pub(crate) cold_start_count: Arc<AtomicU32>,
    pub(super) docker: Docker,
    /// Tracks `session_id -> (lease_id, authorized_keys line)` for SSH sessions
    /// attached to this worker's containers, so detach can strip precisely the
    /// one line it added without disturbing other concurrent sessions on the
    /// same container.
    pub(super) ssh_sessions: Arc<Mutex<HashMap<String, (LeaseId, String)>>>,
    /// Tracks `execution_id -> ExecutionState` for detached commands started
    /// via [`Self::run_command_for_lease_detached`], so a later `poll_command`
    /// can retrieve the running/completed status. Entries are not evicted on
    /// completion, so repeated polls after completion keep returning the same
    /// output.
    pub(super) executions: Arc<Mutex<HashMap<ExecutionId, ExecutionState>>>,
    /// `execution_id -> ProcessMeta` for detached commands started with a
    /// [`ProcessSpec`]. Locked before `executions` when both are needed.
    pub(super) processes: Arc<Mutex<HashMap<ExecutionId, ProcessMeta>>>,
    /// Loopback forwards for ports opened after create, keyed by lease id
    /// then container port. Dropped on release.
    pub(super) port_forwards: Arc<Mutex<HashMap<String, HashMap<u16, crate::port_forward::PortForward>>>>,
    /// `execution_id -> StoredLaunch` for detached commands, so a restart
    /// reuses the original argv, cwd, env and process spec.
    pub(super) launches: Arc<Mutex<HashMap<ExecutionId, StoredLaunch>>>,
    /// Checkpoints and watches over each lease's container files, keyed by lease id.
    pub(super) tracking: Arc<trogon_sandbox_file_ops::Tracking>,
    pub(super) inotify_missing: Arc<crate::file_ops::InotifyMissing>,
    /// Live inotify feeds behind file watches, by watch id.
    pub(super) watch_feeds: Arc<crate::file_ops::feed::WatchFeeds>,
    /// Durable copy of each lease's request, for rollback after a restart.
    pub(super) rollback_state: RollbackStateStore,
}

impl DockerWorkerProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: LocalDockerProvider,
        default_image: String,
        js: jetstream::Context,
        nats_url: String,
        worker_id: String,
        oci_client: OciClient,
        default_registry: String,
        cosign_verifier: Option<CosignVerifier>,
        digest_cache: Arc<DigestCache>,
        volume_region: Option<VolumeRegion>,
        volume_endpoint: Option<VolumeEndpoint>,
        warm_pool_config: WarmPoolConfig,
    ) -> anyhow::Result<Self> {
        let fuse_config = FuseMountConfig {
            region: volume_region.clone(),
            endpoint: volume_endpoint.clone(),
            other_users: FuseOtherUsers::Allowed,
            ..FuseMountConfig::default()
        };
        let fuse_manager = Arc::new(FuseMountManager::new(fuse_config, FuseMetrics::new()?));
        let docker = Docker::connect_with_defaults()
            .context("connect to the Docker socket for exec, file and ssh operations")?;
        Ok(Self {
            dispatch_ledger: None,
            intake_ready: AtomicBool::new(false),
            intake_notify: tokio::sync::Notify::new(),
            command_runtime: None,
            command_custody: command_custody::Custody::new(js.clone()),
            controlled_executions: Mutex::new(HashMap::new()),
            native_custody: Mutex::new(HashMap::new()),
            authority_gates: std::sync::Mutex::new(HashMap::new()),
            authority_paused: Mutex::new(HashSet::new()),
            inner,
            default_image,
            browser_support: None,
            js,
            nats_url,
            worker_id,
            oci_client,
            default_registry,
            cosign_verifier,
            digest_cache,
            volume_region,
            volume_endpoint,
            warm_pool: Arc::new(Mutex::new(WarmPool::new(warm_pool_config))),
            active: Arc::new(Mutex::new(HashMap::new())),
            fuse_manager,
            fuse_handles: Arc::new(Mutex::new(HashMap::new())),
            warm_grab_count: Arc::new(AtomicU32::new(0)),
            cold_start_count: Arc::new(AtomicU32::new(0)),
            docker,
            ssh_sessions: Arc::new(Mutex::new(HashMap::new())),
            executions: Arc::new(Mutex::new(HashMap::new())),
            processes: Arc::new(Mutex::new(HashMap::new())),
            port_forwards: Arc::new(Mutex::new(HashMap::new())),
            launches: Arc::new(Mutex::new(HashMap::new())),
            tracking: Arc::new(trogon_sandbox_file_ops::Tracking::new()),
            inotify_missing: Arc::default(),
            watch_feeds: Arc::default(),
            rollback_state: RollbackStateStore::disabled(),
        })
    }

    pub fn with_browser_support(mut self, support: Option<crate::browser::BrowserSupport>) -> Self {
        self.browser_support = support;
        self
    }

    pub fn with_command_runtime(mut self, runtime: command_runtime::CommandRuntime) -> Self {
        self.command_runtime = Some(Arc::new(runtime));
        self
    }

    pub async fn shutdown_command_runtime(&self) -> anyhow::Result<()> {
        if let Some(runtime) = &self.command_runtime {
            runtime.shutdown(&self.docker).await?;
        }
        Ok(())
    }

    pub fn browser_support(&self) -> Option<&crate::browser::BrowserSupport> {
        self.browser_support.as_ref()
    }

    /// Keeps lease requests on disk so an in-place rollback still works after
    /// a worker restart. Without it the requests live in memory only.
    pub fn with_rollback_state(mut self, store: RollbackStateStore) -> Self {
        self.rollback_state = store;
        self
    }

    pub fn warm_pool(&self) -> Arc<Mutex<WarmPool>> {
        Arc::clone(&self.warm_pool)
    }

    pub fn warm_grab_count(&self) -> u32 {
        self.warm_grab_count.load(Ordering::Relaxed)
    }

    pub fn cold_start_count(&self) -> u32 {
        self.cold_start_count.load(Ordering::Relaxed)
    }

    pub fn cached_digests(&self) -> Vec<Digest> {
        let mut digests = self.digest_cache.digests();
        if let Ok(pool) = self.warm_pool.try_lock() {
            for digest in pool.warm_digests() {
                if !digests.contains(&digest) {
                    digests.push(digest);
                }
            }
        }
        digests
    }

    pub async fn warm_pool_population(&self) -> HashMap<Digest, u32> {
        self.warm_pool.lock().await.population()
    }
}
