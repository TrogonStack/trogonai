pub mod constants;
pub mod file_ops;
pub mod network_policy_needs;
mod profile;
pub mod profile_satisfaction;

pub use constants::PROFILE;
pub use file_ops::WorkspaceRegistry;
#[cfg(target_os = "linux")]
pub use profile::fs_second_layer;
pub use profile::profile_for_backend;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod admission;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod attribution;
mod backend_kind;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod bridge_io;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod config;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod credential_sources;
#[cfg(any(target_os = "macos", windows))]
pub mod fenced_proxy;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod fs_violation;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod git_safe_directories;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod heartbeat;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod isolation;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod launch;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod lease_launch;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod lease_proxy;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod lease_secrets;
#[cfg(target_os = "linux")]
pub mod namespace_bridge;
pub mod placeholders;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod port_forward;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod proxy_env;
#[cfg(target_os = "linux")]
pub mod relay_egress;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod resource_limits;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod ssh_bridge;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod subprocess;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod terminal;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod tool_proxy_env;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
pub mod windows_ca;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod windows_fs;
#[cfg(windows)]
pub mod windows_worker;
#[cfg(unix)]
pub mod worker_authority;

pub use backend_kind::IsolationBackendKind;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use config::{FuseConfig, FuseConfigError, WorkerConfig};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use isolation::{IsolationBackend, LaunchedProcess};
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub use launch::{EgressPolicy, ExtraBind, LaunchSpec};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use subprocess::SubprocessWorker;
#[cfg(windows)]
pub use windows_worker::{WindowsWorker, WindowsWorkerConfig};
