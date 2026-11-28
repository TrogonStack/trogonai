use std::error::Error;
use std::future::Future;
use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    KillCommandResponse, LeaseId, PollCommandResponse, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind,
    SnapshotExportError, SnapshotExportOutcome, WorkerId, WriteStdinResponse,
};
use trogon_sandbox_protocol::subjects::{provider_dispatch, provider_token};
use trogon_sandbox_protocol::{
    Provider, decode_verified, dispatch_failure_reply, nats_transfer, queue_subscribe_verified, subscribe_verified,
};

use crate::constants::LEASE_QUEUE_GROUP;
use crate::{HlukProvider, lease_files};

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

/// What every message handler shares.
struct Worker {
    client: Client,
    provider: Arc<HlukProvider>,
    worker_id: WorkerId,
    dispatch_subject: String,
}

/// Serves lease dispatches and the verbs of held leases on NATS until `cancel` fires.
pub async fn serve(
    client: Client,
    provider: Arc<HlukProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
) -> Result<(), async_nats::SubscribeError> {
    let worker = Arc::new(Worker {
        dispatch_subject: provider_dispatch(ProviderKind::Hluk),
        client,
        provider,
        worker_id,
    });
    let dispatches =
        queue_subscribe_verified(&worker.client, worker.dispatch_subject.clone(), LEASE_QUEUE_GROUP).await?;
    let verbs = subscribe_verified(
        &worker.client,
        format!("sandbox.provider.{}.*.>", provider_token(ProviderKind::Hluk)),
    )
    .await?;
    let mut messages = futures::stream::select(dispatches, verbs);
    tracing::info!(worker_id = %worker.worker_id, "hluk worker ready");
    loop {
        let message = tokio::select! {
            _ = cancel.cancelled() => break,
            message = messages.next() => message,
        };
        let Some(message) = message else { break };
        let worker = Arc::clone(&worker);
        tokio::spawn(async move {
            if let Err(error) = worker.route(message).await {
                tracing::warn!(%error, "hluk worker failed to handle a message");
            }
        });
    }
    Ok(())
}

/// Decodes a request body, runs `handle` on it and encodes the reply.
async fn answer<Req, Resp, Fut>(body: &[u8], handle: impl FnOnce(Req) -> Fut) -> serde_json::Result<Vec<u8>>
where
    Req: DeserializeOwned,
    Resp: Serialize,
    Fut: Future<Output = Resp>,
{
    serde_json::to_vec(&handle(serde_json::from_slice(body)?).await)
}

impl Worker {
    async fn route(&self, message: Message) -> HandlerResult {
        if message.subject.as_str() == self.dispatch_subject {
            return self.grant(message).await;
        }
        // sandbox.provider.hluk.<verb>.<lease_id>
        let mut tokens = message.subject.as_str().split('.').skip(3);
        let (Some(verb), Some(lease_id)) = (tokens.next(), tokens.next()) else {
            return Ok(());
        };
        let lease_id = LeaseId::new(lease_id);
        if verb == "cancel" {
            return Ok(self.provider.cancel(&lease_id).await?);
        }
        // Verbs reach every worker; only the one holding the lease answers.
        let (Some(lease), Some(reply)) = (self.provider.lease_of(&lease_id), message.reply.clone()) else {
            return Ok(());
        };
        let body = nats_transfer::receive(&self.client, &message).await?;
        let out = match verb {
            "run_command" => answer(&body, |request| self.provider.run_command(&lease, request)).await?,
            "write_files" => answer(&body, |request| lease_files::write_files(&lease, request)).await?,
            "read_file" => answer(&body, |request| lease_files::read_file(&lease, request)).await?,
            "file_op" => answer(&body, |request| lease_files::file_op(&lease, request)).await?,
            // No command outlives its request, so there is never one to poll or kill.
            "poll_command" => serde_json::to_vec(&PollCommandResponse::NotFound)?,
            "kill_command" => serde_json::to_vec(&KillCommandResponse::NotFound)?,
            "write_stdin" => serde_json::to_vec(&WriteStdinResponse::Unsupported)?,
            "checkpoint_running" => {
                serde_json::to_vec(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
                    message: "hluk exports no lease snapshot".to_owned(),
                }))?
            }
            _ => return Ok(()),
        };
        nats_transfer::respond(&self.client, reply.as_str(), Bytes::from(out)).await?;
        Ok(())
    }

    async fn grant(&self, message: Message) -> HandlerResult {
        let Some(reply) = message.reply else {
            return Ok(());
        };
        let reply_body = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
            Err(error) => ProviderDispatchReply::DispatchRejected {
                decode_error: error.to_string(),
            },
            Ok(payload) => match self.provider.lease(&payload.request, &payload.lease_id).await {
                Ok(mut lease) => {
                    lease.worker_id = Some(self.worker_id.clone());
                    ProviderDispatchReply::Granted(lease)
                }
                Err(error) => {
                    tracing::warn!(lease_id = %payload.lease_id, %error, "lease refused");
                    dispatch_failure_reply(&error)
                }
            },
        };
        self.client
            .publish(reply, serde_json::to_vec(&reply_body)?.into())
            .await?;
        Ok(())
    }
}
