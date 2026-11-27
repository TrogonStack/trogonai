use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{SnapshotExportError, SnapshotExportOutcome, WorkerId};
use trogon_sandbox_protocol::{subjects, subscribe_verified};

use crate::DaytonaWorkerError;
use crate::constants::UNSUPPORTED;

/// Answers snapshot export requests addressed to this worker with a typed unsupported error, so a caller gets a
/// reason instead of waiting for a timeout.
pub(crate) async fn serve_snapshot_export(
    client: &Client,
    worker_id: &WorkerId,
    cancel: &CancellationToken,
) -> Result<(), DaytonaWorkerError> {
    let mut subscription = subscribe_verified(client, subjects::worker_snapshot_export(worker_id)).await?;
    let client = client.clone();
    let cancel = cancel.clone();
    let body = Bytes::from(serde_json::to_vec(&SnapshotExportOutcome::Err(
        SnapshotExportError::Unsupported {
            message: UNSUPPORTED.to_owned(),
        },
    ))?);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = subscription.next() => {
                    let Some(message) = next else { break };
                    let Some(reply) = message.reply else { continue };
                    let _ = client.publish(reply, body.clone()).await.inspect_err(|e| warn!(error = %e, "failed to reply to snapshot export"));
                }
            }
        }
    });
    Ok(())
}
