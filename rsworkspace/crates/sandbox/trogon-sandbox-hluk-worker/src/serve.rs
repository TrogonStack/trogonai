use std::error::Error;
use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    FileOpResponse, KillCommandResponse, LeaseId, PollCommandResponse, ProviderDispatchPayload, ProviderDispatchReply,
    ProviderKind, ReadFileRequest, RunCommandRequest, SnapshotExportError, SnapshotExportOutcome, WorkerId,
    WriteFilesRequest, WriteStdinResponse,
};
use trogon_sandbox_protocol::subjects::{provider_dispatch, provider_token};
use trogon_sandbox_protocol::{
    Provider, decode_verified, dispatch_failure_reply, nats_transfer, queue_subscribe_verified, subscribe_verified,
};

use crate::HlukProvider;
use crate::constants::LEASE_QUEUE_GROUP;

type HandlerResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Serves lease dispatches and the verbs of held leases on NATS until `cancel` fires.
pub async fn serve(
    client: Client,
    provider: Arc<HlukProvider>,
    worker_id: WorkerId,
    cancel: CancellationToken,
) -> Result<(), async_nats::SubscribeError> {
    let token = provider_token(ProviderKind::Hluk);
    let dispatches =
        queue_subscribe_verified(&client, provider_dispatch(ProviderKind::Hluk), LEASE_QUEUE_GROUP).await?;
    let verbs = subscribe_verified(&client, format!("sandbox.provider.{token}.*.>")).await?;
    let mut messages = futures::stream::select(dispatches, verbs);
    tracing::info!(%worker_id, "hluk worker ready");
    loop {
        let message = tokio::select! {
            _ = cancel.cancelled() => break,
            message = messages.next() => message,
        };
        let Some(message) = message else { break };
        let (client, provider, worker_id) = (client.clone(), Arc::clone(&provider), worker_id.clone());
        tokio::spawn(async move {
            if let Err(error) = route(&client, &provider, &worker_id, message).await {
                tracing::warn!(%error, "hluk worker failed to handle a message");
            }
        });
    }
    Ok(())
}

async fn route(client: &Client, provider: &HlukProvider, worker_id: &WorkerId, message: Message) -> HandlerResult {
    if message.subject.as_str() == provider_dispatch(ProviderKind::Hluk) {
        return grant(client, provider, worker_id, message).await;
    }
    // sandbox.provider.hluk.<verb>.<lease_id>
    let mut tokens = message.subject.as_str().split('.').skip(3);
    let (Some(verb), Some(lease_id)) = (tokens.next(), tokens.next()) else {
        return Ok(());
    };
    let lease_id = LeaseId::new(lease_id);
    if verb == "cancel" {
        return Ok(provider.cancel(&lease_id).await?);
    }
    // Verbs reach every worker; only the one holding the lease answers.
    let (Some(lease), Some(reply)) = (provider.lease_of(&lease_id), message.reply.clone()) else {
        return Ok(());
    };
    let body = nats_transfer::receive(client, &message).await?;
    let out = match verb {
        "run_command" => serde_json::to_vec(
            &provider
                .run_command(&lease, &serde_json::from_slice::<RunCommandRequest>(&body)?)
                .await,
        )?,
        "write_files" => serde_json::to_vec(
            &provider
                .write_files(&lease, &serde_json::from_slice::<WriteFilesRequest>(&body)?)
                .await,
        )?,
        "read_file" => serde_json::to_vec(
            &provider
                .read_file(&lease, &serde_json::from_slice::<ReadFileRequest>(&body)?)
                .await,
        )?,
        // No command outlives its request, so there is never one to poll or kill.
        "poll_command" => serde_json::to_vec(&PollCommandResponse::NotFound)?,
        "kill_command" => serde_json::to_vec(&KillCommandResponse::NotFound)?,
        "write_stdin" => serde_json::to_vec(&WriteStdinResponse::Unsupported)?,
        "file_op" => serde_json::to_vec(&FileOpResponse::Unsupported {
            message: "hluk serves only write_files and read_file".to_owned(),
        })?,
        "checkpoint_running" => serde_json::to_vec(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
            message: "hluk exports no lease snapshot".to_owned(),
        }))?,
        _ => return Ok(()),
    };
    nats_transfer::respond(client, reply.as_str(), Bytes::from(out)).await?;
    Ok(())
}

async fn grant(client: &Client, provider: &HlukProvider, worker_id: &WorkerId, message: Message) -> HandlerResult {
    let Some(reply) = message.reply else {
        return Ok(());
    };
    let answer = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
        Err(error) => ProviderDispatchReply::DispatchRejected {
            decode_error: error.to_string(),
        },
        Ok(payload) => match provider.lease(&payload.request, &payload.lease_id).await {
            Ok(mut lease) => {
                lease.worker_id = Some(worker_id.clone());
                ProviderDispatchReply::Granted(lease)
            }
            Err(error) => {
                tracing::warn!(lease_id = %payload.lease_id, %error, "lease refused");
                dispatch_failure_reply(&error)
            }
        },
    };
    client.publish(reply, serde_json::to_vec(&answer)?.into()).await?;
    Ok(())
}
