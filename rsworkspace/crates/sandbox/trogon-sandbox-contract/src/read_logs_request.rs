use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId};

/// A client request to read a streamed command's retained output in pages.
///
/// `after_seq` is the `log_seq` of the last chunk the caller has seen, and 0 reads from the start, so a client that
/// reloads resumes where it left off.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadLogsRequest {
    pub lease_id: LeaseId,
    pub execution_id: ExecutionId,
    #[serde(default)]
    pub after_seq: u64,
    /// The page size in bytes. Unset reads [`ReadLogsRequest::DEFAULT_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Long poll: when no chunk follows `after_seq` yet, hold the reply for up to this many milliseconds, bounded by
    /// [`ReadLogsRequest::MAX_WAIT_MS`], until one arrives. Unset or 0 replies at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
}

impl ReadLogsRequest {
    /// The page size applied when `max_bytes` is unset.
    pub const DEFAULT_BYTES: u64 = 64 * 1024;
    /// The most bytes of output one page may return.
    pub const MAX_BYTES: u64 = 512 * 1024;
    /// The longest one long poll may hold a reply, in milliseconds.
    pub const MAX_WAIT_MS: u64 = 20_000;
}
