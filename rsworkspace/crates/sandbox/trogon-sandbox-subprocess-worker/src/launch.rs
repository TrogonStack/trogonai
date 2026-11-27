use std::path::PathBuf;

use trogon_sandbox_contract::{FsPolicy, PlatformOptions};

use crate::fs_violation::FsDenialSink;

#[cfg(target_os = "linux")]
use crate::relay_egress::RelayEgress;
use crate::resource_limits::ResourceLimits;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPolicy {
    DenyAll,
    AllowAll,
    /// An empty network namespace whose only way out is the relay to this
    /// lease's egress proxy.
    #[cfg(target_os = "linux")]
    Relay(RelayEgress),
    /// The host network, narrowed to this one loopback address, where the
    /// lease's egress proxy listens.
    #[cfg(any(target_os = "macos", windows))]
    Loopback(std::net::SocketAddr),
}

#[derive(Debug, Clone)]
pub struct ExtraBind {
    pub host_path: PathBuf,
    pub sandbox_path: String,
    pub readonly: bool,
    pub credential_mask: bool,
}

#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub workspace_host_path: PathBuf,
    pub workspace_sandbox_path: PathBuf,
    pub workspace_readonly: bool,
    pub egress_policy: EgressPolicy,
    pub extra_binds: Vec<ExtraBind>,
    pub user: Option<String>,
    pub limits: ResourceLimits,
    pub fs_policy: Option<FsPolicy>,
    /// Opt-ins the backend must enforce or refuse.
    pub platform: PlatformOptions,
    /// Where the backend reports filesystem operations the sandbox refused,
    /// when it can observe them.
    pub fs_denials: Option<FsDenialSink>,
}
