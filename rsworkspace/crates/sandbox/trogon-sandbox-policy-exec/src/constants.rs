//! Fixed values the crate's modules share.

#[cfg(unix)]
use std::time::Duration;

/// The shortest id a caller may give a command.
#[cfg(unix)]
pub const MIN_COMMAND_ID_LEN: usize = 22;
#[cfg(unix)]
pub(crate) const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 16 * 1024 * 1024;

#[cfg(all(unix, any(target_os = "linux", test)))]
pub(crate) const CAP_SETFCAP: u32 = 31;
#[cfg(all(unix, any(target_os = "linux", test)))]
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_millis(5000);
#[cfg(all(unix, any(target_os = "linux", test)))]
pub(crate) const OLDEST_FULLY_SUPPORTED_BWRAP_VERSION: &str = "0.5.0";
#[cfg(all(unix, any(target_os = "linux", test)))]
pub(crate) const CAP_SETFCAP_MISSING_MESSAGE: &str = "running as uid 0 without CAP_SETFCAP in this process's capability bounding set - on kernels that enforce the CAP_SETFCAP requirement for mapping uid 0 into a user namespace (Linux 5.12 and distribution backports) every sandboxed command fails while writing a uid map (\"Operation not permitted\"). Grant CAP_SETFCAP to this process, or run as a non-root user";

#[cfg(unix)]
pub(crate) const SANDBOX_MARKER: (&str, &str) = ("TROGON_SANDBOXED", "1");
#[cfg(unix)]
pub(crate) const DEFAULT_TMPDIR: &str = "/tmp/sandbox";

pub(crate) const TENANT: &str = "local";

pub(crate) const HOST_SOURCE_TTL_SECONDS: u32 = 60;

#[cfg(unix)]
pub(crate) const LISTEN_BACKLOG: libc::c_int = 511;
#[cfg(unix)]
pub(crate) const PROXY_ONLY_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

#[cfg(unix)]
pub(crate) const GUEST_WORKSPACE: &str = "/workspace";
#[cfg(unix)]
pub(crate) const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
pub(crate) const VIOLATION_STREAM_WAIT: Duration = Duration::from_secs(5);
#[cfg(unix)]
pub(crate) const RESET_WAIT: Duration = Duration::from_secs(10);
#[cfg(unix)]
pub(crate) const RESET_POLL: Duration = Duration::from_millis(20);

/// Windows has no always-writable devices.
#[cfg(windows)]
pub(crate) const DEFAULT_WRITE_DEVICES: [&str; 0] = [];
#[cfg(windows)]
pub(crate) const DEFAULT_WRITE_DIRS: [&str; 0] = [];

/// The directories under the home directory that are writable beside the
/// default ones, for the tools a command runs.
#[cfg(unix)]
pub(crate) const HOME_CONVENIENCE_WRITE_DIRS: [&str; 2] = [".npm/_logs", ".agent/debug"];
#[cfg(windows)]
pub(crate) const HOME_CONVENIENCE_WRITE_DIRS: [&str; 0] = [];

#[cfg(unix)]
pub(crate) const CA_FILE_NAME: &str = "ca.pem";
#[cfg(unix)]
pub(crate) const PROXY_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
#[cfg(unix)]
pub(crate) const SOCKS5_VERSION: u8 = 0x05;

/// How many directories a `safe.directory` wildcard replaces.
#[cfg(any(windows, test))]
pub(crate) const SAFE_DIRECTORY_WILDCARD_THRESHOLD: usize = 8;
