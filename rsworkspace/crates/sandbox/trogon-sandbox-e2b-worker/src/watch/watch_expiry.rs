use std::sync::Arc;

use super::watch_entry::WatchEntry;
use super::watches::Watches;
use crate::envd::EnvdTarget;

impl Watches {
    /// Removes the lease's watches that nobody polled for the idle expiry.
    pub(super) async fn expire_idle(&self, lease: &str, target: &EnvdTarget) {
        let candidates: Vec<(String, Arc<WatchEntry>)> = self
            .leases()
            .get(lease)
            .map(|held| held.iter().map(|(id, watch)| (id.clone(), Arc::clone(watch))).collect())
            .unwrap_or_default();
        for (id, watch) in candidates {
            // A watch with a poll in flight holds its buffer lock and is not idle.
            let idle = watch
                .buffer
                .try_lock()
                .is_ok_and(|buffer| buffer.touched.elapsed() >= self.idle_expiry);
            if idle {
                self.forget(lease, &id);
                self.remove_remote(target, &watch).await;
            }
        }
    }
}
