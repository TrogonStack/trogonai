use super::*;

#[derive(Clone)]
pub(super) struct ActiveDockerLease {
    pub(super) runtime_mount: Option<command_runtime::RuntimeMount>,
    pub(super) registration_id: String,
    pub(super) dispatch_target: Option<trogon_sandbox_contract::DispatchTarget>,
    pub(super) dispatch_target_bound: bool,
    pub(super) container_id: String,
    pub(super) snapshot_digest: Option<Digest>,
    pub(super) snapshot_reference: Option<OciArtifactRef>,
    pub(super) memory_mib: u32,
    pub(super) cpu: f32,
    /// Host path of the egress sidecar's live policy file, if this lease's
    /// container was launched with a `NetworkPolicy::UserDefined` (only that
    /// case starts a sidecar). `None` for warm-pool reuse and checkpoint
    /// restores, which never launch one.
    pub(super) policy_file_path: Option<PathBuf>,
    /// Container id of the egress sidecar. Killing the workload leaves the
    /// sidecar running, so teardown removes it explicitly.
    pub(super) sidecar_id: Option<String>,
    /// Maps a container-side port to a reachable `host:port` endpoint, for
    /// leases whose `SandboxSpec::exposed_ports` was non-empty at launch
    /// time. Empty for warm-pool reuse and checkpoint restores, which do not
    /// go through a fresh port-publishing launch.
    pub(super) exposed_port_endpoints: HashMap<u16, String>,
    /// Whether this container is safe to reset in place and return to the
    /// warm pool on `cancel()`, i.e. it was either taken from the pool or
    /// cold-started via a snapshot lease whose workspace is
    /// `workspace_allows_warm_pool`-eligible (empty workspace, no host bind
    /// mounts -- see `workspace_allows_warm_pool`). Snapshot leases with a
    /// real `HostMount`/`Clone` workspace, and live checkpoint restores (whose
    /// PID 1 is the checkpointed workload, not `sleep infinity`), must always
    /// go through full teardown: resetting them would either leak a
    /// host-mounted tenant directory into the pool or leave the previous
    /// tenant's process running for the next lease.
    pub(super) warm_pool_eligible: bool,
    /// The request this lease was created from, kept in memory so an in-place
    /// rollback can rebuild the container with the same shape. Never persisted.
    pub(super) request: Box<LeaseRequest>,
}
