use std::time::Duration;

/// The port envd listens on inside every sandbox.
pub(crate) const ENVD_PORT: u16 = 49983;

/// How long a connection may take to establish.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Extra time a request may outlive the command it carries.
pub(crate) const REQUEST_GRACE: Duration = Duration::from_secs(10);

/// Largest frame a stream reader accepts; a bigger declared length ends the stream, so a
/// misbehaving peer cannot make the reader buffer without bound.
pub(crate) const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// How long a PTY process may live, sent as the Connect timeout and used as the request
/// deadline. Matches the longest E2B sandbox lifetime.
pub(crate) const PTY_LIFETIME: Duration = Duration::from_secs(86_400);

/// Largest `filesystem.Filesystem` reply accepted.
pub(crate) const MAX_FILESYSTEM_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// Most events kept from one `GetWatcherEvents` reply.
pub const MAX_WATCHER_EVENTS_PER_FETCH: usize = 4096;

/// How long a unary call (input, resize, signal) may take.
pub(crate) const UNARY_TIMEOUT: Duration = Duration::from_secs(10);

/// Bytes in a Connect frame header: one flag byte and a four byte big endian length.
pub(crate) const FRAME_HEADER_BYTES: usize = 5;

/// Flag bit of the frame that ends a Connect stream.
pub(crate) const END_STREAM_FLAG: u8 = 0x02;

/// Header that carries the sandbox's access token.
pub(crate) const ACCESS_TOKEN_HEADER: &str = "X-Access-Token";

pub(crate) const CONNECT_VERSION_HEADER: &str = "Connect-Protocol-Version";
pub(crate) const CONNECT_VERSION: &str = "1";
pub(crate) const CONNECT_TIMEOUT_HEADER: &str = "Connect-Timeout-Ms";
pub(crate) const KEEPALIVE_HEADER: &str = "Keepalive-Ping-Interval";

/// Asks the edge for periodic keepalive events so an idle terminal is not cut.
pub(crate) const KEEPALIVE_SECS: &str = "50";

pub(crate) const CONNECT_STREAM_CONTENT_TYPE: &str = "application/connect+json";

/// The program every command runs under, so the argv stays exact and a missing program is
/// exit 127.
pub(crate) const BASH_PROGRAM: &str = "/bin/bash";
pub(crate) const BASH_PREFIX_ARGS: [&str; 4] = ["-l", "-c", "exec \"$@\"", "bash"];

/// The signal that kills a process.
pub(crate) const SIGKILL: &str = "SIGNAL_SIGKILL";

/// HTTP status reported for a Connect error the worker maps to a rejected request.
pub(crate) const REJECTED_STATUS: u16 = 400;

/// File name sent with an upload whose path has none.
pub(crate) const FALLBACK_UPLOAD_NAME: &str = "file";

/// Marker of the 502 body the edge answers when the sandbox no longer exists.
pub(crate) const SANDBOX_GONE_MARKER: &str = "not found";

/// Exit status reported when the process ended without a clean exit.
pub(crate) const NO_CLEAN_EXIT: i32 = -1;
