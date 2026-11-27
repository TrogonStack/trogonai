use std::time::Duration;

/// Directory whose state at lease start is the baseline for `changes` without a checkpoint: the home of the user E2B
/// runs commands as.
pub const BASELINE_PATH: &str = "/home/user";

/// Largest manifest script output accepted, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

/// How long the lease-start scan may take before the lease goes without a baseline.
pub const BASELINE_TIMEOUT: Duration = Duration::from_secs(20);

/// How long one manifest scan may take on request.
pub(super) const SCAN_TIMEOUT: Duration = Duration::from_secs(60);

/// The user the lease-start scan runs as.
pub(super) const BASELINE_USER: &str = "user";

pub(super) const EXIT_NOT_FOUND: i32 = 44;
pub(super) const EXIT_NO_COMMAND: i32 = 127;

// $1 path, $2 recursive flag, $3 line ceiling, $4 byte ceiling. Both ceilings are one past the accepted maximum so
// the caller can tell a full tree from a cut one.
pub(super) const MANIFEST_SCRIPT: &str = r#"
[ -e "$1" ] || [ -L "$1" ] || exit 44
if [ -d "$1" ] && [ ! -L "$1" ]; then
  if [ "$2" = 1 ]; then
    find "$1" -mindepth 1 -exec stat -c '%F|%s|%y|%n' {} + 2>/dev/null | head -n "$3" | head -c "$4"
  else
    find "$1" -mindepth 1 -maxdepth 1 -exec stat -c '%F|%s|%y|%n' {} + 2>/dev/null | head -n "$3" | head -c "$4"
  fi
else
  stat -c '%F|%s|%y|%n' "$1"
fi
"#;

pub(super) const HASH_SCRIPT: &str = r#"
command -v sha256sum >/dev/null 2>&1 || exit 127
if [ -d "$1" ] && [ ! -L "$1" ]; then
  if [ "$2" = 1 ]; then
    find "$1" -type f -exec sha256sum {} + 2>/dev/null | head -n "$3" | head -c "$4"
  else
    find "$1" -mindepth 1 -maxdepth 1 -type f -exec sha256sum {} + 2>/dev/null | head -n "$3" | head -c "$4"
  fi
else
  sha256sum "$1"
fi
"#;
