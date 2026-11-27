use tokio_util::sync::CancellationToken;

use super::buffer::Buffer;

/// One watcher envd holds for a lease, with the events buffered from it.
pub(super) struct WatchEntry {
    pub(super) envd_id: String,
    pub(super) root: String,
    pub(super) user: String,
    pub(super) buffer: tokio::sync::Mutex<Buffer>,
    /// Cancelled when the watch is stopped, expired or its lease released, which ends a poll parked on it.
    pub(super) gone: CancellationToken,
}
