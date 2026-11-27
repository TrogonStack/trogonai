#[cfg(unix)]
use rustix::fs::OFlags;

/// Entry count ceiling for one archive.
pub(crate) const MAX_ARCHIVE_ENTRIES: u64 = 100_000;

/// Compiled-regex size ceiling, so a hostile pattern cannot exhaust the worker.
pub(crate) const REGEX_SIZE_LIMIT: usize = 1024 * 1024;

/// Longest line kept in a content match; longer lines are cut so one minified file cannot flood a
/// reply.
pub(crate) const MAX_MATCH_LINE_CHARS: usize = 500;

/// Total bytes one content search reads across files.
pub(crate) const MAX_SEARCH_SCAN_BYTES: u64 = 256 * 1024 * 1024;

#[cfg(unix)]
pub(crate) const WRITE_FLAGS: OFlags = OFlags::WRONLY.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
