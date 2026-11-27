use std::num::{NonZeroU32, NonZeroUsize};
use std::time::Duration;

/// Most sessions one worker bridges at once.
pub const DEFAULT_MAX_SESSIONS: NonZeroUsize = NonZeroUsize::MIN.saturating_add(63);

/// A session with no traffic in either direction for this long is closed.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Most bytes of client input one session may have received but not yet written to its terminal. Exceeding it ends the
/// session.
pub const DEFAULT_MAX_BUFFERED_BYTES: usize = 1024 * 1024;

/// Most channels (terminals) one session may have open.
pub const DEFAULT_MAX_CHANNELS: NonZeroUsize = NonZeroUsize::MIN.saturating_add(3);

pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) const KEEPALIVE_MAX: usize = 3;

/// Where the gateway keeps its host key unless told otherwise.
pub const DEFAULT_HOST_KEY_PATH: &str = "data/trogon-sandbox-ssh-gateway/host_key";

/// Frames the gateway buffers per session toward the SSH client. A full buffer stops reading the subscription, so a
/// slow client is throttled by NATS rather than by unbounded memory.
pub(crate) const FRAME_BUFFER: usize = 256;

pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const READ_BUFFER_BYTES: usize = 8 * 1024;

/// How long a terminal whose output ended waits for the guest to report its exit status.
pub(crate) const EXIT_CODE_WAIT: Duration = Duration::from_secs(5);

pub(crate) const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(3600);

pub(crate) const AUTH_REJECTION_TIME: Duration = Duration::from_secs(1);

/// Most channel bytes the bridge puts in one frame.
pub(crate) const MAX_FRAME_DATA_BYTES: usize = 8 * 1024;

/// Longest `TERM` the bridge passes on.
pub(crate) const MAX_TERM_LEN: usize = 64;

/// The end-of-transmission byte a PTY turns into end of input.
pub(crate) const EOT: u8 = 0x04;

/// Lifetime requested for a session and its broker token.
pub const DEFAULT_SESSION_TTL_SECONDS: NonZeroU32 = NonZeroU32::MIN.saturating_add(3599);

/// Window assumed for a channel whose client did not request a pty.
pub(crate) const DEFAULT_COLS: u16 = 80;

pub(crate) const DEFAULT_ROWS: u16 = 24;

/// Exit status reported when the worker could not serve a channel or the process died without one.
pub(crate) const FAILURE_EXIT_STATUS: u32 = 255;

/// Most bytes of client input in one frame.
pub(crate) const MAX_FRAME_BYTES: usize = 8 * 1024;

/// Longest lifetime a session may have, whatever `ttl_seconds` it asks for.
pub const MAX_SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Bytes buffered between the SSH channel and a terminal in each direction.
pub(crate) const PIPE_BYTES: usize = 64 * 1024;

pub(crate) const RESIZE_QUEUE: usize = 8;

/// Every attach subject, for a worker that serves any lease.
pub const SSH_ATTACH_FILTER: &str = "sandbox.ssh.attach.>";

/// Every detach subject.
pub const SSH_DETACH_FILTER: &str = "sandbox.ssh.detach.>";

pub(crate) const INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// A terminal with no input and no output for this long is closed.
pub const DEFAULT_TERMINAL_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(crate) const DEFAULT_TERM: &str = "xterm-256color";
