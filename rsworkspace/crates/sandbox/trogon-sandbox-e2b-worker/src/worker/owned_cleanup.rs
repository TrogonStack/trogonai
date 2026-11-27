use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{CancelOwnedLeaseRequest, ProviderKind};
use trogon_sandbox_protocol::{subjects, subscribe_verified};

use crate::E2bWorkerProvider;
use crate::worker::E2bWorkerError;
use crate::worker::constants::CANCEL_OWNED;
use crate::worker::reply::publish_json;

/// The subject a cancel-owned request for `request`'s lease must arrive on.
pub(crate) fn owned_subject(request: &CancelOwnedLeaseRequest) -> String {
    subjects::provider_cancel_owned(ProviderKind::E2b, &request.lease_id)
}

/// Answers requests to cancel a lease this worker owns. Only the worker that owns the sandbox replies, and a request
/// that arrives on a subject other than the one its own lease names is dropped.
pub(crate) async fn serve_owned_cleanup(
    client: &Client,
    e2b: &Arc<E2bWorkerProvider>,
    cancel: &CancellationToken,
) -> Result<(), E2bWorkerError> {
    let mut subscription = subscribe_verified(client, CANCEL_OWNED).await?;
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
                        let Ok(request) = serde_json::from_slice::<CancelOwnedLeaseRequest>(&message.payload) else {
                            return;
                        };
                        if message.subject.as_str() != owned_subject(&request) {
                            return;
                        }
                        let Some(response) = e2b.cancel_owned(&request).await else { return };
                        publish_json(&client, reply, &response, "owned cleanup").await;
                    });
                }
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
