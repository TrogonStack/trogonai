use std::time::SystemTime;

use async_nats::Subscriber;
use futures::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::LeaseReleased;

use crate::SnapshotBuilderMetrics;

/// Reads lease releases that ask for a snapshot and queues them for processing.
pub(crate) async fn forward_releases(
    mut subscription: Subscriber,
    queue: Sender<(LeaseReleased, SystemTime)>,
    cancel: CancellationToken,
    metrics: SnapshotBuilderMetrics,
) {
    loop {
        let message = tokio::select! {
            _ = cancel.cancelled() => break,
            message = subscription.next() => match message {
                Some(message) => message,
                None => break,
            },
        };
        let event: LeaseReleased = match serde_json::from_slice(&message.payload) {
            Ok(event) => event,
            Err(error) => {
                warn!(%error, "ignoring malformed LeaseReleased payload");
                metrics.failed_total.with_label_values(&["invalid_payload"]).inc();
                continue;
            }
        };
        if event.snapshot_out.is_none() {
            continue;
        }
        // Arrival is the compatibility fallback for older brokers.
        if queue.send((event, SystemTime::now())).await.is_err() {
            break;
        }
        let depth = queue.max_capacity().saturating_sub(queue.capacity());
        metrics.queue_depth.set(i64::try_from(depth).unwrap_or(i64::MAX));
    }
}
