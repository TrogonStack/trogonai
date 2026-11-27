use std::time::Duration;

/// Broadcast subject the broker sends terminal requests on for this provider
/// (see [`trogon_sandbox_protocol::subjects::provider_terminal`]).
pub const TERMINAL_SUBJECT: &str = "sandbox.provider.e2b.terminal.>";

/// The unprivileged user E2B sandboxes run as.
pub(super) const TERMINAL_USER: &str = "user";

/// The shell every terminal runs, as a login shell.
pub(super) const TERMINAL_SHELL: &str = "/bin/bash";

/// Prefix of the unique tag each terminal's process carries.
pub(super) const TAG_PREFIX: &str = "trogon-term-";

/// How long a new terminal waits for envd to report the process started.
pub(super) const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Capacity of each pipe between the hub and the envd calls.
pub(super) const PIPE_BYTES: usize = 64 * 1024;

/// Largest piece of input sent in one `SendInput`.
pub(super) const INPUT_CHUNK_BYTES: usize = 8 * 1024;

/// Pending resizes; the latest sizes win, so a handful is plenty.
pub(super) const RESIZE_QUEUE: usize = 8;
