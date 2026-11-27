use std::sync::Arc;
use std::time::Duration;

use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;

use crate::AcaWorkerProvider;

/// Retries the cleanup of failed allocations every `period` until `cancel` fires.
pub async fn run_cleanup_retries(provider: Arc<AcaWorkerProvider>, period: Duration, cancel: CancellationToken) {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => provider.retry_failed_allocations().await,
        }
    }
}
