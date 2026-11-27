use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{LeaseError, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse, ProviderKind};
use trogon_sandbox_protocol::{queue_subscribe_verified, subjects};

use crate::E2bWorkerProvider;
use crate::worker::E2bWorkerError;
use crate::worker::constants::{INVALID_NATIVE_SNAPSHOT_DELETE, NATIVE_SNAPSHOT_DELETE_QUEUE_GROUP};
use crate::worker::reply::publish_json;

/// The refusal for a deletion request that does not decode.
pub(crate) fn invalid_request(error: &serde_json::Error) -> NativeSnapshotDeleteResponse {
    NativeSnapshotDeleteResponse::Denied(LeaseError::Backend {
        message: format!("{INVALID_NATIVE_SNAPSHOT_DELETE}: {error}"),
    })
}

/// Answers requests to delete a native E2B snapshot. Workers share one queue group, so exactly one takes each.
pub(crate) async fn serve_native_snapshot_delete(
    client: &Client,
    e2b: &Arc<E2bWorkerProvider>,
    cancel: &CancellationToken,
) -> Result<(), E2bWorkerError> {
    let mut subscription = queue_subscribe_verified(
        client,
        subjects::provider_native_snapshot_delete(ProviderKind::E2b),
        NATIVE_SNAPSHOT_DELETE_QUEUE_GROUP,
    )
    .await?;
    let (client, e2b, cancel) = (client.clone(), Arc::clone(e2b), cancel.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = subscription.next() => {
                    let Some(message) = next else { break };
                    let (client, e2b) = (client.clone(), Arc::clone(&e2b));
                    tokio::spawn(async move {
                        let Some(reply) = message.reply else { return };
                        let response = match serde_json::from_slice::<NativeSnapshotDeleteRequest>(&message.payload) {
                            Ok(request) => e2b.native_snapshot_delete(&request).await,
                            Err(error) => invalid_request(&error),
                        };
                        publish_json(&client, reply, &response, "native snapshot delete").await;
                    });
                }
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
