use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_contract::{
    ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind,
    WorkerId,
};
use trogon_sandbox_protocol::{
    VerifiedRequest, decode_verified, nats_transfer, subjects, subscribe_verified, subscribe_verified_context,
};

use crate::cleanup_scope::{INVALID_CLEANUP_SCOPE, names_assignment};
use crate::{VercelWorkerError, VercelWorkerProvider};

/// Serves the assignments addressed to this worker alone: dispatches on its intake subject and cleanups on its cleanup
/// subject. A worker with no dispatch ledger takes no assignments.
pub(crate) async fn serve_targeted(
    client: &Client,
    js: &jetstream::Context,
    vercel: &Arc<VercelWorkerProvider>,
    cancel: &CancellationToken,
) -> Result<(), VercelWorkerError> {
    let Some(ledger) = vercel.dispatch_ledger() else {
        return Ok(());
    };
    let worker_id = ledger.worker_id().clone();
    let mut dispatches =
        subscribe_verified(client, subjects::worker_dispatch(ProviderKind::Vercel, &worker_id)).await?;
    let mut cleanups =
        subscribe_verified_context(client, subjects::worker_cleanup(ProviderKind::Vercel, &worker_id)).await?;
    let (client, js, vercel, cancel) = (client.clone(), js.clone(), Arc::clone(vercel), cancel.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = dispatches.next() => {
                    let Some(message) = next else { break };
                    let (client, js, vercel, worker_id) = (client.clone(), js.clone(), Arc::clone(&vercel), worker_id.clone());
                    tokio::spawn(async move {
                        let _ = answer_dispatch(&client, &js, &vercel, &worker_id, message)
                            .await
                            .inspect_err(|error| warn!(%error, "failed to answer a targeted dispatch"));
                    });
                }
                next = cleanups.next() => {
                    let Some(verified) = next else { break };
                    let (client, vercel) = (client.clone(), Arc::clone(&vercel));
                    tokio::spawn(async move {
                        let _ = answer_cleanup(&client, &vercel, verified)
                            .await
                            .inspect_err(|error| warn!(%error, "failed to answer a targeted cleanup"));
                    });
                }
            }
        }
    });
    Ok(())
}

async fn answer_dispatch(
    client: &Client,
    js: &jetstream::Context,
    vercel: &VercelWorkerProvider,
    worker_id: &WorkerId,
    message: async_nats::Message,
) -> Result<(), VercelWorkerError> {
    let reply = match decode_verified::<ProviderDispatchPayload>(&message.payload, Some(worker_id)) {
        Err(error) => ProviderDispatchReply::DispatchRejected {
            decode_error: error.to_string(),
        },
        Ok(dispatch) => match vercel.lease_dispatch(&dispatch, Some(js)).await {
            Ok(lease) => ProviderDispatchReply::Granted(lease),
            Err(error) => ProviderDispatchReply::LaunchFailed {
                provider_error: error.to_string(),
            },
        },
    };
    if let Some(subject) = message.reply {
        client.publish(subject, serde_json::to_vec(&reply)?.into()).await?;
    }
    Ok(())
}

/// Answers one cleanup for an assignment of this worker. A cleanup deletes the sandbox its receipt names, even after a
/// restart lost the worker's own record of it, so it runs only for a request signed for exactly the lease, worker,
/// attempt and epoch it names; any other gets a failure.
async fn answer_cleanup(
    client: &Client,
    vercel: &VercelWorkerProvider,
    verified: VerifiedRequest,
) -> Result<(), VercelWorkerError> {
    let VerifiedRequest { message, scope } = verified;
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let bytes = nats_transfer::receive(client, &message).await?;
    let request: ProviderCleanupRequest = serde_json::from_slice(&bytes)?;
    let exact = scope.is_some_and(|scope| {
        scope.matches(message.subject.as_str(), &bytes) && names_assignment(scope.scope(), &request)
    });
    let response = if exact {
        vercel.cleanup_dispatch(&request).await
    } else {
        ProviderCleanupResponse::Failed {
            identity: request.identity.clone(),
            message: INVALID_CLEANUP_SCOPE.to_owned(),
        }
    };
    client.publish(reply, serde_json::to_vec(&response)?.into()).await?;
    Ok(())
}
