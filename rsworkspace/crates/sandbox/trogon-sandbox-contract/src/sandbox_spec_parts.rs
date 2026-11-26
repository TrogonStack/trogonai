use std::collections::BTreeMap;
use std::num::NonZeroU32;

use crate::{
    BootMode, DriveMount, FsPolicy, IdlePolicy, LeaseTtl, NetworkPolicy, PlatformOptions, Port, PrivilegedAccess,
    RemoteMount, RetentionToggles, SandboxImage, SecretBindings, SnapshotPolicy, WasmCapabilityGrant, WorkspaceSource,
};

/// The fields of a [`SandboxSpec`](crate::SandboxSpec) before its rules are checked.
///
/// Hand them to [`SandboxSpec::new`](crate::SandboxSpec::new).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SandboxSpecParts {
    pub image: SandboxImage,
    pub workspace: WorkspaceSource,
    pub env: BTreeMap<String, String>,
    pub cpu_seconds: NonZeroU32,
    pub memory_mib: NonZeroU32,
    pub snapshot_out: Option<SnapshotPolicy>,
    pub network_policy: Option<NetworkPolicy>,
    pub mounts: Vec<RemoteMount>,
    pub drives: Vec<DriveMount>,
    pub exposed_ports: Vec<Port>,
    pub boot_mode: BootMode,
    pub working_dir: Option<String>,
    pub user: Option<String>,
    pub privileged: PrivilegedAccess,
    pub wasi_capabilities: Option<WasmCapabilityGrant>,
    pub fs_policy: Option<FsPolicy>,
    pub secret_bindings: SecretBindings,
    pub platform: PlatformOptions,
    pub idle_timeout_seconds: Option<NonZeroU32>,
    pub no_ttl: LeaseTtl,
    pub idle_policy: Option<IdlePolicy>,
    pub vcpu_millis: Option<u32>,
    pub disk_mib: Option<u32>,
    pub retention: RetentionToggles,
}
