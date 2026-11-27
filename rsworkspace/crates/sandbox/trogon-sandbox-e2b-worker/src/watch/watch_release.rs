use std::sync::Arc;

use futures::future::join_all;
use tracing::warn;

use super::watch_entry::WatchEntry;
use super::watches::Watches;
use crate::envd::EnvdTarget;

impl Watches {
    /// Drops a released lease's watches and removes their watchers from envd, waiting a bounded time so a paused or
    /// dead sandbox cannot stall the release.
    pub async fn release(&self, lease: &str, target: &EnvdTarget) {
        let watches: Vec<Arc<WatchEntry>> = self
            .leases()
            .remove(lease)
            .map(|held| held.into_values().collect())
            .unwrap_or_default();
        let removals = join_all(watches.iter().map(|watch| self.remove_remote(target, watch)));
        if tokio::time::timeout(self.release_timeout, removals).await.is_err() {
            warn!(lease_id = %lease, "timed out removing envd watchers on release");
        }
    }
}
