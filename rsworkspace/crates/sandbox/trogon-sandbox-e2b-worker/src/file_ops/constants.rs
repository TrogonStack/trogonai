//! Limits, deadlines, exit codes, and guest scripts of the file operations.

use std::time::Duration;

/// Longest one guest shell helper runs before it is treated as failed.
pub(super) const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest an archive extraction runs inside the guest.
pub(super) const EXTRACT_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest one search or archive collection takes in total.
pub(super) const COLLECTION_DEADLINE: Duration = Duration::from_secs(120);

/// Longest the destination preflight of an archive upload takes.
pub(super) const PREFLIGHT_DEADLINE: Duration = Duration::from_secs(120);

/// Characters of guest stderr carried into a failure message.
pub(super) const STDERR_EXCERPT_CHARS: usize = 300;

/// Content bytes one search stages on the host before it stops.
pub(super) const MAX_SEARCH_BYTES: u64 = 256 * 1024 * 1024;

/// Symlinks followed while resolving the root of a collection.
pub(super) const MAX_SYMLINK_HOPS: usize = 40;

/// Longest relative path, in bytes, a collection accepts.
pub(super) const MAX_RELATIVE_PATH_BYTES: usize = 4096;

/// Most segments a relative path of a collection holds.
pub(super) const MAX_RELATIVE_PATH_SEGMENTS: usize = 256;

/// Size of one tar block.
pub(super) const TAR_BLOCK_BYTES: u64 = 512;

/// Size of the two zero blocks that end a tar stream.
pub(super) const TAR_TRAILER_BYTES: u64 = 1024;

/// Longest name a plain tar header holds before a long-name record is added.
pub(super) const TAR_NAME_BYTES: usize = 100;

/// Seconds in the millisecond timestamps the guest reports.
pub(super) const MILLIS_PER_SECOND: u64 = 1000;

/// Permission bits a staged archive entry keeps.
pub(super) const ARCHIVE_MODE_MASK: u32 = 0o777;

/// Permission bits an exported archive entry keeps.
pub(super) const EXPORT_MODE_MASK: u32 = 0o7777;

/// Mode of an exported directory the guest reported no mode for.
pub(super) const DEFAULT_DIRECTORY_MODE: u32 = 0o755;

/// Mode of an exported file the guest reported no mode for.
pub(super) const DEFAULT_FILE_MODE: u32 = 0o644;

/// Exit code of the guest scripts when the target is a directory where a file is needed.
pub(super) const EXIT_IS_DIRECTORY: i32 = 21;

/// Exit code of the edit script when the file changed after it was read.
pub(super) const EXIT_CHANGED: i32 = 73;

/// Exit code of the path scripts when the entry already exists.
pub(super) const EXIT_EXISTS: i32 = 17;

/// Exit code of the path scripts when the source is missing.
pub(super) const EXIT_MISSING: i32 = 44;

/// Exit code of the path scripts when the target is the sandbox root.
pub(super) const EXIT_ROOT: i32 = 64;

/// Exit code of the path scripts when the entry exists already as a directory or is absent.
pub(super) const EXIT_NOOP: i32 = 10;

/// Exit code of a shell that could not find a required tool.
pub(super) const EXIT_NO_TOOL: i32 = 127;

pub(super) const MKDIR: &str = r#"path=$1; parents=$2
if [ -d "$path" ]; then exit 10; fi
if [ -e "$path" ] || [ -L "$path" ]; then exit 17; fi
if [ "$parents" = 1 ]; then
  parent=$(dirname -- "$path") || exit 1
  mkdir -p -- "$parent" || exit 1
fi
mkdir -- "$path" && exit 0
[ -d "$path" ] && exit 10
exit 1
"#;

pub(super) const REMOVE: &str = r#"path=$1; recursive=$2
if [ ! -e "$path" ] && [ ! -L "$path" ]; then exit 10; fi
if [ ! -L "$path" ] && [ -d "$path" ] && [ "$(cd -- "$path" && pwd -P)" = / ]; then exit 64; fi
if [ -d "$path" ] && [ ! -L "$path" ]; then
  if [ "$recursive" = 1 ]; then rm -r -- "$path"; else rmdir -- "$path"; fi
else
  rm -- "$path"
fi
"#;

pub(super) const RENAME_NO_REPLACE: &str = r#"source=$1; target=$2
if [ ! -e "$source" ] && [ ! -L "$source" ]; then exit 44; fi
if [ -e "$target" ] || [ -L "$target" ]; then exit 17; fi
if [ ! -L "$source" ] && [ -d "$source" ] && [ "$(cd -- "$source" && pwd -P)" = / ]; then exit 64; fi
source_device=$(stat -c %d -- "$source") || exit 1
target_parent=$(dirname -- "$target") || exit 1
target_device=$(stat -c %d -- "$target_parent") || exit 1
if [ "$source_device" != "$target_device" ]; then echo 'cross-device rename is not supported' >&2; exit 1; fi
mv -T -n -- "$source" "$target" || exit $?
if [ -e "$source" ] || [ -L "$source" ]; then exit 17; fi
"#;

pub(super) const WRITE_SCRIPT: &str = r#"path=$1; temporary=$2; offset=$3; truncate=$4; mode=$5
trap 'rm -f -- "$temporary" "$temporary.err"' EXIT
[ ! -d "$path" ] || exit 21
if [ "$truncate" = 1 ]; then : > "$path" || exit 1; fi
if dd if=/dev/null of=/dev/null oflag=seek_bytes count=0 2>/dev/null; then
  dd if="$temporary" of="$path" bs=65536 seek="$offset" oflag=seek_bytes conv=notrunc 2>"$temporary.err"
else
  dd if="$temporary" of="$path" bs=1 seek="$offset" conv=notrunc 2>"$temporary.err"
fi
status=$?
if [ "$status" != 0 ]; then cat "$temporary.err" >&2; exit "$status"; fi
if [ -n "$mode" ]; then chmod -- "$mode" "$path" || exit 1; fi
"#;

pub(super) const EDIT: &str = r#"path=$1; original=$2; replacement=$3; staged=
trap 'rm -f -- "$original" "$replacement"; [ -z "$staged" ] || rm -f -- "$staged"' EXIT
resolved=$(readlink -f -- "$path") || exit 1
[ -f "$resolved" ] || exit 21
cmp -s -- "$original" "$resolved" || exit 73
parent=$(dirname -- "$resolved") || exit 1
staged=$(mktemp "$parent/.trogon-edit-XXXXXXXX") || exit 1
cat -- "$replacement" > "$staged" || exit 1
cp --attributes-only --preserve=mode,ownership -- "$resolved" "$staged" || exit 1
cmp -s -- "$original" "$resolved" || exit 73
mv -T -f -- "$staged" "$resolved"
"#;

pub(super) const EXTRACT: &str = r#"archive=$1; target=$2
trap 'rm -f -- "$archive" "$archive.err"' EXIT
mkdir -p -- "$target" || exit 1
tar --extract --file="$archive" --directory="$target" --no-same-owner --same-permissions --no-overwrite-dir 2>"$archive.err"
status=$?
head -c 4096 -- "$archive.err" >&2
exit "$status"
"#;
