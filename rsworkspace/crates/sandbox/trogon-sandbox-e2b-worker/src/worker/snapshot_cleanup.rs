use std::sync::Arc;

use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;

use crate::E2bWorkerProvider;
use crate::worker::constants::SNAPSHOT_CLEANUP_INTERVAL;
use crate::worker::log_exit;

/// Retries the native snapshot deletions still pending, every [`SNAPSHOT_CLEANUP_INTERVAL`], until `cancel` fires.
pub(crate) fn spawn_snapshot_cleanup(e2b: Arc<E2bWorkerProvider>, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut ticks = interval(SNAPSHOT_CLEANUP_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = ticks.tick() => {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        result = e2b.recover_snapshot_cleanup() => {
                            log_exit("native snapshot cleanup recovery", result);
                        }
                    }
                }
            }
        }
    });
}
