use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{FenceView, WorkerFenceStore};

/// Refreshes `view` from `store` every `interval` until `cancel` fires,
/// starting with an immediate load.
pub fn spawn_fence_refresh(
    store: Arc<dyn WorkerFenceStore>,
    view: FenceView,
    interval: Duration,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => view.refresh(store.as_ref()).await,
                _ = cancel.cancelled() => break,
            }
        }
    });
}
