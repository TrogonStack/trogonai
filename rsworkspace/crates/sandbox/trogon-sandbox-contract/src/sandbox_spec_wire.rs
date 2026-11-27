use std::collections::BTreeMap;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::{
    BootMode, DriveMount, FsPolicy, IdlePolicy, LeaseTtl, NetworkPolicy, PlatformOptions, Port, PrivilegedAccess,
    RemoteMount, RetentionToggles, SandboxImage, SecretBindings, SnapshotPolicy, WasmCapabilityGrant, WorkspaceSource,
};

/// The JSON shape of a [`SandboxSpec`](crate::SandboxSpec) before its rules are checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SandboxSpecWire {
    pub(crate) image: SandboxImage,
    pub(crate) workspace: WorkspaceSource,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) cpu_seconds: NonZeroU32,
    pub(crate) memory_mib: NonZeroU32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot_out: Option<SnapshotPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) network_policy: Option<NetworkPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) mounts: Vec<RemoteMount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) drives: Vec<DriveMount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) exposed_ports: Vec<Port>,
    #[serde(default)]
    pub(crate) boot_mode: BootMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) user: Option<String>,
    #[serde(default)]
    pub(crate) privileged: PrivilegedAccess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wasi_capabilities: Option<WasmCapabilityGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fs_policy: Option<FsPolicy>,
    #[serde(default, skip_serializing_if = "SecretBindings::is_empty")]
    pub(crate) secret_bindings: SecretBindings,
    #[serde(default, skip_serializing_if = "PlatformOptions::is_default")]
    pub(crate) platform: PlatformOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) idle_timeout_seconds: Option<NonZeroU32>,
    #[serde(default, skip_serializing_if = "LeaseTtl::is_default")]
    pub(crate) no_ttl: LeaseTtl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) idle_policy: Option<IdlePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) vcpu_millis: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) disk_mib: Option<u32>,
    #[serde(default, skip_serializing_if = "RetentionToggles::is_default")]
    pub(crate) retention: RetentionToggles,
}
