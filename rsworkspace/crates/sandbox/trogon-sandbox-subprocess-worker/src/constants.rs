use std::sync::LazyLock;
#[cfg(unix)]
use std::time::Duration;

use trogon_sandbox_contract::ProviderCapabilityProfile;

/// What the worker advertises for a backend that confines the payload.
pub static PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(crate::profile::base_profile);

/// What a Windows worker advertises once the network fence is installed.
#[cfg(windows)]
pub static FENCED_PROFILE: LazyLock<ProviderCapabilityProfile> = LazyLock::new(crate::profile::fenced_profile);

/// Where the lease's workspace appears inside the sandbox.
pub const SANDBOX_WORKSPACE_PATH: &str = "/workspace";

/// How long a cancel waits for the lease's process to be confirmed gone.
#[cfg(unix)]
pub(crate) const CANCEL_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a lease's process is told to die while it is still alive.
#[cfg(unix)]
pub(crate) const TERMINATION_POLL: Duration = Duration::from_millis(100);

/// Largest file a masked file binding will read to locate its credential.
pub(crate) const MAX_SOURCE_FILE_BYTES: u64 = 1024 * 1024;

/// Why a masked file is withheld instead of replaced with a fake copy.
#[cfg(target_os = "macos")]
pub(crate) const MASK_DEGRADES_TO_DENY: &str = "file mask degrades to deny on macOS until the interposer lands";
/// Why a masked file is withheld instead of replaced with a fake copy.
#[cfg(not(target_os = "macos"))]
pub(crate) const MASK_DEGRADES_TO_DENY: &str =
    "file mask degrades to deny on this platform: the sandbox cannot read the file";

/// Whether this platform can bind a fake copy over a masked file.
pub(crate) const MASKED_FILES_SUPPORTED: bool = cfg!(target_os = "linux");

/// Most of git's stderr kept for an error message.
pub(crate) const GIT_STDERR_CAP: usize = 64 * 1024;

pub(crate) const POLICY_FILE_NAME: &str = "policy.json";
#[cfg(target_os = "linux")]
pub(crate) const SOCKET_NAME: &str = "proxy.sock";
pub(crate) const CA_FILE_NAME: &str = "ca.pem";
pub(crate) const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) const READY_POLL: std::time::Duration = std::time::Duration::from_millis(20);
pub(crate) const TOKEN_BYTES: usize = 16;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const INPUT_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a hung-up command gets to exit before it is killed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const HANGUP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const DEFAULT_SHELL: &str = "/bin/sh";
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const DEFAULT_TERM: &str = "xterm-256color";

pub(crate) const BASE_USER: &str = "sandbox";
pub(crate) const KEY_UNITS: usize = 100;
pub(crate) const MAX_USER_BYTES: usize = 255;

/// Most ports one lease can open; matches the docker worker.
pub const MAX_FORWARDS_PER_LEASE: usize = 16;

/// Most connections one forward bridges at once. Each is a helper process, so
/// an unbounded client could otherwise exhaust the worker's pids.
pub const MAX_BRIDGES_PER_FORWARD: usize = 32;

pub(crate) const GUEST_CA_PATH: &str = "/etc/sandbox-egress-proxy-ca/ca.pem";

/// Hosts the tool variables keep off the proxy. `*.local` is absent, so that
/// names the sandbox cannot resolve go through the proxy. Set only with the
/// tool variables: a library user's policy can allow a private range, and a
/// bypass list would send that traffic around the proxy.
#[cfg(unix)]
pub(crate) const NO_PROXY: &str = "localhost,127.0.0.1,::1,169.254.0.0/16,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16";

pub(crate) const SSH_MUX_OVERRIDE: &str = "-o ControlMaster=no -o ControlPath=none";
pub(crate) const BSD_NC: &str = "/usr/bin/nc";

pub const SSH_PROXY_SCRIPT_VAR: &str = "SANDBOX_SSH_PROXY_COMMAND";
pub const JAVA_TOOL_OPTIONS: &str = "JAVA_TOOL_OPTIONS";
pub const PREFER_IPV4_STACK: &str = "-Djava.net.preferIPv4Stack=true";
pub(crate) const GIT_BASIC_PROXY_AUTH: &str = "'http.proxyAuthMethod=basic'";

pub(crate) static MISSING_AGENT_LOGGED: std::sync::Once = std::sync::Once::new();
