use trogon_sandbox_contract::FileOp;

/// Largest archive stream staged out of a container for one operation.
pub(crate) const STAGING_CAP_BYTES: u64 = FileOp::MAX_ARCHIVE_BYTES;

/// Bytes hashed per ranged read while computing a checksum.
pub(super) const CHECKSUM_PAGE_BYTES: u64 = FileOp::MAX_READ_BYTES;

pub(super) const EXIT_NOT_FOUND: i32 = 44;
pub(super) const EXIT_IS_DIRECTORY: i32 = 21;
pub(super) const EXIT_EXISTS: i32 = 17;
pub(super) const EXIT_OFFSET_MISMATCH: i32 = 22;
pub(super) const EXIT_NO_TAR: i32 = 127;
pub(super) const EXIT_NO_COMMAND: i32 = 127;

pub(super) const MANIFEST_SCRIPT: &str = r#"
[ -e "$1" ] || [ -L "$1" ] || exit 44
if [ -d "$1" ] && [ ! -L "$1" ]; then
  if [ "$2" = 1 ]; then
    find "$1" -mindepth 1 -exec stat -c '%F|%s|%y|%n' {} + 2>/dev/null | head -n "$3"
  else
    find "$1" -mindepth 1 -maxdepth 1 -exec stat -c '%F|%s|%y|%n' {} + 2>/dev/null | head -n "$3"
  fi
else
  stat -c '%F|%s|%y|%n' "$1"
fi
"#;

pub(super) const HASH_SCRIPT: &str = r#"
command -v sha256sum >/dev/null 2>&1 || exit 127
if [ -d "$1" ] && [ ! -L "$1" ]; then
  if [ "$2" = 1 ]; then
    find "$1" -type f -exec sha256sum {} + 2>/dev/null | head -n "$3"
  else
    find "$1" -mindepth 1 -maxdepth 1 -type f -exec sha256sum {} + 2>/dev/null | head -n "$3"
  fi
else
  sha256sum "$1"
fi
"#;

/// Directory whose state at lease start is the baseline for `changes` without a checkpoint.
pub(crate) const WORKSPACE_PATH: &str = "/workspace";

/// Longest one `inotifywait` blocks. A change landing between the previous scan and the
/// watch being set up is not reported by it, so the wait is kept short enough that such
/// a change still shows up within a few seconds.
pub(super) const INOTIFY_MAX_WAIT_SECONDS: u64 = 5;

/// Exit code of [`INOTIFY_WAIT_SCRIPT`] when the image has no `inotifywait`.
pub(super) const INOTIFY_MISSING_EXIT: i32 = 127;

/// Exits 127 when the image has no `inotifywait`, otherwise blocks until the first
/// change under `$1` (recursively when `$2` is 1) or `$3` seconds pass.
pub(super) const INOTIFY_WAIT_SCRIPT: &str = r#"
command -v inotifywait >/dev/null 2>&1 || exit 127
if [ "$2" = 1 ]; then
  exec inotifywait -qq -r -t "$3" -e modify,attrib,close_write,move,create,delete "$1"
fi
exec inotifywait -qq -t "$3" -e modify,attrib,close_write,move,create,delete "$1"
"#;

/// Hints a feed keeps for a watch nobody is polling. Past it the oldest are dropped, which
/// only loses what the scans cannot see.
pub(super) const MAX_PENDING_HINTS: usize = 4096;

/// Prints its own pid (so the worker can stop it), then streams one `EVENTS|PATH` line per
/// change under `$1`, recursively when `$2` is 1. Exits 127 without `inotifywait`.
pub(super) const FEED_SCRIPT: &str = r#"
command -v inotifywait >/dev/null 2>&1 || exit 127
echo "pid $$"
if [ "$2" = 1 ]; then
  exec inotifywait -m -q -r -e modify,attrib,close_write,move,create,delete --format '%e|%w%f' "$1"
fi
exec inotifywait -m -q -e modify,attrib,close_write,move,create,delete --format '%e|%w%f' "$1"
"#;
