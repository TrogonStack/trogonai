use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::WarmPoolManager;
use crate::constants::WARM_POOL_RECONCILE_INTERVAL;

/// Reconciles the warm pool with the cluster and evicts idle VMs every [`WARM_POOL_RECONCILE_INTERVAL`] until
/// `stop` is cancelled. A disabled pool is left alone.
pub(crate) async fn run_warm_pool_loop(pool: Arc<WarmPoolManager>, stop: CancellationToken) {
    let mut ticker = tokio::time::interval(WARM_POOL_RECONCILE_INTERVAL);
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            _ = ticker.tick() => {}
        }
        if !pool.config().enabled {
            continue;
        }
        if let Err(error) = pool.reconcile_from_cluster().await {
            warn!(%error, "warm pool reconciliation failed");
        }
        pool.evict_idle().await;
    }
}
