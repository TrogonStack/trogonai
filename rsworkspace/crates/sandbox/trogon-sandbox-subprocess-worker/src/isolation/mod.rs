#[cfg(windows)]
pub mod appcontainer;
mod constants;

#[cfg(unix)]
mod backend;
#[cfg(target_os = "linux")]
mod bubblewrap;
#[cfg(target_os = "linux")]
mod bubblewrap_version;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod default_writes;
#[cfg(target_os = "linux")]
mod diagnostics;
#[cfg(target_os = "macos")]
mod exit_cleanup;
mod factory;
#[cfg(any(windows, all(test, target_os = "macos")))]
pub mod fenced;
#[cfg(target_os = "linux")]
pub mod guest_init;
#[cfg(target_os = "linux")]
pub mod helper;
#[cfg(target_os = "linux")]
pub mod host_tree;
#[cfg(unix)]
mod host_view;
#[cfg(target_os = "linux")]
pub mod landlock;
#[cfg(unix)]
pub mod limits;
#[cfg(unix)]
#[cfg_attr(
    not(target_os = "linux"),
    expect(
        dead_code,
        reason = "only the Linux mount planner and the macOS profile use every entry"
    )
)]
mod mandatory;
#[cfg(target_os = "linux")]
pub mod mount_plan;
#[cfg(target_os = "linux")]
pub mod nested;
#[cfg(target_os = "linux")]
pub mod observer;
#[cfg(unix)]
mod passthrough;
#[cfg(unix)]
mod platform_support;
pub mod profile_journal;
#[cfg(target_os = "linux")]
pub mod relay;
pub use crate::resource_limits;
#[cfg(target_os = "macos")]
pub mod seatbelt;
#[cfg(target_os = "linux")]
mod seccomp;
#[cfg(target_os = "linux")]
mod userns;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod win_command;

#[cfg(target_os = "linux")]
pub use bubblewrap::BubblewrapBackend;
#[cfg(unix)]
pub use host_view::HostView;
#[cfg(unix)]
pub use passthrough::PassthroughBackend;
#[cfg(unix)]
pub(crate) use platform_support::ensure_enforceable as ensure_platform_options_enforceable;
#[cfg(target_os = "macos")]
pub use seatbelt::SeatbeltBackend;

#[cfg(target_os = "linux")]
pub(crate) use backend::share_network;
#[cfg(unix)]
pub use backend::{ChildStdio, IsolationBackend, LaunchedProcess, MonitorGuard, StdioMode, TerminalStdio};
#[cfg(unix)]
pub use factory::build_isolation_backend;
