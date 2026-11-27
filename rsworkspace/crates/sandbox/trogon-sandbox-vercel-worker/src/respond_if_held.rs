use std::future::Future;
use std::sync::Arc;

use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_protocol::{nats_transfer, subscribe_verified};

use crate::lease_ref::LeaseRef;
use crate::{LeaseSandbox, VercelWorkerError, VercelWorkerProvider};

/// Subscribes `subject` and answers each request on it with `handler`, for as long as `cancel` stays unfired. With
/// `transfer`, the request and the reply may span several bus messages.
pub(crate) async fn serve<Req, H, Fut>(
    client: &Client,
    vercel: &Arc<VercelWorkerProvider>,
    cancel: &CancellationToken,
    subject: &'static str,
    transfer: bool,
    handler: H,
) -> Result<(), VercelWorkerError>
where
    Req: DeserializeOwned + Send + 'static,
    H: Fn(Arc<VercelWorkerProvider>, LeaseSandbox, Req, usize) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<u8>, VercelWorkerError>> + Send,
{
    let mut subscription = subscribe_verified(client, subject).await?;
    let client = client.clone();
    let vercel = Arc::clone(vercel);
    let cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = subscription.next() => {
                    let Some(message) = next else { break };
                    let (client, vercel) = (client.clone(), Arc::clone(&vercel));
                    tokio::spawn(async move {
                        if let Err(error) = respond_if_held(message, client, vercel, transfer, handler).await {
                            warn!(%error, "failed to handle lease verb");
                        }
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
    vercel: Arc<VercelWorkerProvider>,
    transfer: bool,
    handler: H,
) -> Result<(), VercelWorkerError>
where
    Req: DeserializeOwned,
    H: Fn(Arc<VercelWorkerProvider>, LeaseSandbox, Req, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, VercelWorkerError>>,
{
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let Ok(head) = serde_json::from_slice::<LeaseRef>(&payload) else {
        warn!("invalid lease verb request");
        return Ok(());
    };
    let Some(sandbox) = vercel.sandbox_for_lease(&head.lease_id).await else {
        return Ok(());
    };
    let Ok(request) = serde_json::from_slice::<Req>(&payload) else {
        warn!("invalid lease verb request");
        return Ok(());
    };
    let max_payload = client.server_info().max_payload;
    let body = Bytes::from(handler(vercel, sandbox, request, max_payload).await?);
    if transfer {
        nats_transfer::respond(&client, reply.as_str(), body).await?;
    } else {
        client.publish(reply, body).await?;
    }
    Ok(())
}
