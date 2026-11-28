use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, LeaseError, RunCommandRequest, RunCommandResponse,
};
use trogon_sandbox_protocol::{
    VerifiedRequest, VerifiedVerbScope, fit_run_command_response, nats_transfer, subscribe_verified_context,
};

use crate::E2bWorkerProvider;
use crate::worker::E2bWorkerError;
use crate::worker::constants as subjects;
use crate::worker::log_exit;

pub(crate) fn invalid_command_scope() -> LeaseError {
    LeaseError::InvalidSpec {
        reason: subjects::INVALID_COMMAND_SCOPE.to_owned(),
    }
}

/// Whether a request's signature, when it has one, covers exactly its subject and body.
fn covers(scope: Option<&VerifiedVerbScope>, subject: &str, payload: &[u8]) -> bool {
    scope.is_none_or(|scope| scope.matches(subject, payload))
}

/// Admits a request whose signature, when it has one, covers exactly its subject and body.
pub(crate) fn admit(covered: bool) -> Result<(), LeaseError> {
    if covered { Ok(()) } else { Err(invalid_command_scope()) }
}

/// The scope a request must carry, signed for exactly this subject and body.
pub(crate) fn require_exact_scope<'a>(
    scope: Option<&'a VerifiedVerbScope>,
    subject: &str,
    payload: &[u8],
) -> Result<&'a VerifiedVerbScope, LeaseError> {
    scope
        .filter(|scope| scope.matches(subject, payload))
        .ok_or_else(invalid_command_scope)
}

/// Serves command launches and their cancellations, which carry the verified scope of the dispatch that signed them.
pub(crate) async fn serve_command_launches(
    client: &Client,
    e2b: &Arc<E2bWorkerProvider>,
    cancel: &CancellationToken,
) -> Result<(), E2bWorkerError> {
    let mut run = subscribe_verified_context(client, subjects::RUN_COMMAND).await?;
    let mut stop = subscribe_verified_context(client, subjects::CANCEL_COMMAND_LAUNCH).await?;
    let (client, e2b, cancel) = (client.clone(), Arc::clone(e2b), cancel.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = run.next() => {
                    let Some(request) = next else { break };
                    let (client, e2b) = (client.clone(), Arc::clone(&e2b));
                    tokio::spawn(async move {
                        log_exit("run command response", respond_run_command(request, client, e2b).await);
                    });
                }
                next = stop.next() => {
                    let Some(request) = next else { break };
                    let (client, e2b) = (client.clone(), Arc::clone(&e2b));
                    tokio::spawn(async move {
                        log_exit(
                            "command launch cancellation response",
                            respond_cancel_command_launch(request, client, e2b).await,
                        );
                    });
                }
            }
        }
    });
    Ok(())
}

async fn respond_run_command(
    verified: VerifiedRequest,
    client: Client,
    e2b: Arc<E2bWorkerProvider>,
) -> Result<(), E2bWorkerError> {
    let VerifiedRequest { message, scope } = verified;
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let request: RunCommandRequest = serde_json::from_slice(&payload).map_err(|source| E2bWorkerError::Decode {
        what: subjects::INVALID_RUN_COMMAND,
        source,
    })?;
    let held = async {
        admit(covers(scope.as_ref(), message.subject.as_str(), &payload))?;
        e2b.sandbox_for_command(
            &request.lease_id,
            scope.as_ref(),
            request.require_launch_cancellation.is_required(),
        )
        .await
    }
    .await;
    let response = match held {
        Ok(Some(sandbox)) => match scope.as_ref() {
            Some(scope) => {
                e2b.exec()
                    .handle_run_command_with_scope(&sandbox, &request, scope, &payload)
                    .await
            }
            None => e2b.exec().handle_run_command(&sandbox, &request).await,
        },
        Ok(None) => return Ok(()),
        Err(error) => RunCommandResponse::Denied(error),
    };
    let fitted = fit_run_command_response(response, client.server_info().max_payload);
    client.publish(reply, serde_json::to_vec(&fitted)?.into()).await?;
    Ok(())
}

async fn respond_cancel_command_launch(
    verified: VerifiedRequest,
    client: Client,
    e2b: Arc<E2bWorkerProvider>,
) -> Result<(), E2bWorkerError> {
    let VerifiedRequest { message, scope } = verified;
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let request: CancelCommandLaunchRequest =
        serde_json::from_slice(&payload).map_err(|source| E2bWorkerError::Decode {
            what: subjects::INVALID_CANCEL_COMMAND_LAUNCH,
            source,
        })?;
    let held = async {
        let scope = require_exact_scope(scope.as_ref(), message.subject.as_str(), &payload)?;
        let sandbox = e2b.sandbox_for_command(&request.lease_id, Some(scope), true).await?;
        Ok::<_, LeaseError>((scope, sandbox))
    }
    .await;
    let response = match held {
        Ok((scope, Some(sandbox))) => {
            e2b.exec()
                .handle_cancel_command_launch_with_scope(&sandbox, &request, scope, &payload)
                .await
        }
        Ok((_, None)) => return Ok(()),
        Err(error) => CancelCommandLaunchResponse::Denied(error),
    };
    client.publish(reply, serde_json::to_vec(&response)?.into()).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
