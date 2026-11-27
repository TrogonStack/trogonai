use std::future::Future;
use std::sync::Arc;

use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_protocol::{nats_transfer, subscribe_verified};

use crate::worker::E2bWorkerError;
use crate::worker::lease_ref::LeaseRef;
use crate::worker::log_exit;
use crate::{E2bWorkerProvider, LeaseSandbox};

/// Subscribes `subject` and answers each request on it with `handler`, for as long as `cancel` stays unfired. With
/// `transfer`, the request and the reply may span several bus messages.
pub(crate) async fn serve<Req, H, Fut>(
    client: &Client,
    e2b: &Arc<E2bWorkerProvider>,
    cancel: &CancellationToken,
    subject: &'static str,
    transfer: bool,
    handler: H,
) -> Result<(), E2bWorkerError>
where
    Req: DeserializeOwned + Send + 'static,
    H: Fn(Arc<E2bWorkerProvider>, LeaseSandbox, Req, usize) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<u8>, E2bWorkerError>> + Send,
{
    let mut subscription = subscribe_verified(client, subject).await?;
    let client = client.clone();
    let e2b = Arc::clone(e2b);
    let cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = subscription.next() => {
                    let Some(message) = next else { break };
                    let (client, e2b) = (client.clone(), Arc::clone(&e2b));
                    tokio::spawn(async move {
                        log_exit(
                            "lease verb",
                            respond_if_held(message, client, e2b, transfer, handler).await,
                        );
                    });
                }
            }
        }
    });
    Ok(())
}

/// Decodes a broadcast verb, stays silent unless this worker holds the lease, and publishes the handler's reply.
async fn respond_if_held<Req, H, Fut>(
    message: async_nats::Message,
    client: Client,
    e2b: Arc<E2bWorkerProvider>,
    transfer: bool,
    handler: H,
) -> Result<(), E2bWorkerError>
where
    Req: DeserializeOwned,
    H: Fn(Arc<E2bWorkerProvider>, LeaseSandbox, Req, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, E2bWorkerError>>,
{
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let Ok(head) = serde_json::from_slice::<LeaseRef>(&payload) else {
        warn!("invalid lease verb request");
        return Ok(());
    };
    let Some(sandbox) = e2b.sandbox_for_lease(&head.lease_id).await else {
        return Ok(());
    };
    let Ok(request) = serde_json::from_slice::<Req>(&payload) else {
        warn!("invalid lease verb request");
        return Ok(());
    };
    let max_payload = client.server_info().max_payload;
    let body = Bytes::from(handler(e2b, sandbox, request, max_payload).await?);
    if transfer {
        nats_transfer::respond(&client, reply.as_str(), body).await?;
    } else {
        client.publish(reply, body).await?;
    }
    Ok(())
}
