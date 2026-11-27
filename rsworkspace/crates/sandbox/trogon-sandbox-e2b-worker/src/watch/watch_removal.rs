use tracing::warn;
use trogon_sandbox_contract::{FileOpResponse, FilePresence};

use super::watch_entry::WatchEntry;
use super::watches::Watches;
use crate::api::ApiError;
use crate::envd::EnvdTarget;

impl Watches {
    pub(super) fn forget(&self, lease: &str, watch_id: &str) {
        let removed = self.leases().get_mut(lease).and_then(|held| held.remove(watch_id));
        if let Some(watch) = removed {
            watch.gone.cancel();
        }
    }

    pub(super) async fn remove_remote(&self, target: &EnvdTarget, watch: &WatchEntry) {
        watch.gone.cancel();
        if let Err(error) = self.envd.remove_watcher(target, &watch.user, &watch.envd_id).await
            && !matches!(error, ApiError::NotFound(_))
        {
            warn!(error = %error, "failed to remove an envd watcher");
        }
    }

    pub(super) async fn stop(&self, lease: &str, target: &EnvdTarget, watch_id: &str) -> FileOpResponse {
        let watch = self.leases().get_mut(lease).and_then(|held| held.remove(watch_id));
        if let Some(watch) = &watch {
            self.remove_remote(target, watch).await;
        }
        FileOpResponse::WatchStopped {
            existed: FilePresence::from(watch.is_some()),
        }
    }
}
