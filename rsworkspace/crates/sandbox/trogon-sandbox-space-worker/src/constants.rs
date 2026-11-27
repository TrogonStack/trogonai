//! Limits and timeouts shared across the worker.

use std::time::Duration;

/// Most vCPUs SPACE lets one sandbox request.
pub(crate) const MAX_VCPU: u32 = 32;

pub(crate) const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_ATTEMPTS: u32 = 4;
pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_secs(35);
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const KILL_TIMEOUT: Duration = Duration::from_secs(15);

/// Times a dropped event stream is resumed with `Last-Event-ID`.
pub(crate) const MAX_RESUMES: u32 = 3;
/// Times a refused connection to the event stream is tried again.
pub(crate) const MAX_REFUSED: u32 = 2;

/// File content moves through guest commands in pieces of this size.
pub(crate) const WRITE_CHUNK_BYTES: usize = 24 * 1024;
pub(crate) const READ_CHUNK_BYTES: usize = 512 * 1024;
pub(crate) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_FILES: usize = 256;
pub(crate) const MAX_MODE: u32 = 0o7777;
pub(crate) const DEFAULT_MODE: u32 = 0o644;
/// Most output of a short guest script the worker keeps.
pub(crate) const SCRIPT_OUTPUT_CAP: usize = 64 * 1024;
/// How long one guest script may run, in seconds.
pub(crate) const SCRIPT_TIMEOUT_SECONDS: u32 = 30;
pub(crate) const MAKE_TEMPORARY: &str = "mkdir -p -- \"$(dirname -- \"$1\")\" && mktemp -- \"$1.sandbox.XXXXXX\"";
pub(crate) const APPEND_CHUNK: &str = "printf '%s' \"$2\" | base64 -d >> \"$1\"";
pub(crate) const INSTALL: &str = "chmod -- \"$3\" \"$1\" && mv -f -- \"$1\" \"$2\"";
pub(crate) const REMOVE: &str = "rm -f -- \"$1\"";
pub(crate) const READ_CHUNK: &str = "test -e \"$1\" || exit 3; test -f \"$1\" || exit 1; \
                          dd if=\"$1\" bs=524288 skip=\"$2\" count=1 status=none | base64 -w 0";
pub(crate) const NOT_FOUND_EXIT: i32 = 3;

/// How long in-flight requests may finish after the worker is told to stop.
pub(crate) const DRAIN: Duration = Duration::from_secs(300);

/// Longest unterminated line or event the parser buffers.
pub(crate) const MAX_PENDING_BYTES: usize = 2 * 1024 * 1024;
