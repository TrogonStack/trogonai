use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, LeaseId, NetworkPolicy, PrivilegedAccess};

use crate::{ArtifactSink, CloneCredentials, CommitSha, EgressPolicy, HostMount, ImageRef, RepoRef, ToolSpec};

/// Everything a provider needs to launch one sandbox.
#[derive(Clone)]
pub struct SandboxSpec {
    pub image: ImageRef,
    pub cpu: f32,
    pub memory_mib: u32,
    /// Wall-clock limit the provider enforces on the sandbox. Zero means no
    /// deadline: the lease asked for `no_ttl` and ends only on release or
    /// its idle timeout.
    pub timeout: Duration,
    pub repo: RepoRef,
    pub resolved_sha: CommitSha,
    pub base_sha: Option<CommitSha>,
    pub tools: Vec<ToolSpec>,
    pub artifact_sink: Arc<dyn ArtifactSink>,
    pub egress_policy: EgressPolicy,
    pub network_policy: Option<NetworkPolicy>,
    pub clone: Option<CloneCredentials>,
    pub host_mount: Option<HostMount>,
    pub extra_binds: Vec<String>,
    /// Writable-layer quota in MiB. `None` leaves the engine default.
    pub disk_mib: Option<u32>,
    pub boot_mode: BootMode,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub privileged: PrivilegedAccess,
    pub exposed_ports: Vec<u16>,
    /// Environment the lease asked for. Every engine adapter must hand it to
    /// the sandbox's main process; none may drop it.
    pub env: BTreeMap<String, String>,
    pub lease_id: Option<LeaseId>,
}
