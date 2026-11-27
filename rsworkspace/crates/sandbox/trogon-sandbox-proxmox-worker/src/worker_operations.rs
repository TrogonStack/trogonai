use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use trogon_sandbox_contract::{
    FileOpRequest, LeaseId, ReadFileRequest, RunCommandRequest, RunCommandResponse, WriteFilesRequest,
    WriteFilesResponse,
};
use trogon_sandbox_protocol::{
    fit_run_command_response, nats_transfer, read_file_response, unsupported_file_op_response,
};

use crate::denied_body::DeniedBody;
use crate::provider_edge::denial;
use crate::{ProxmoxProvider, ProxmoxWorkerError};

#[derive(Deserialize)]
struct LeaseRef {
    lease_id: LeaseId,
}

fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProxmoxWorkerError> {
    Ok(serde_json::from_slice(payload)?)
}

fn encode<T: Serialize>(response: &T) -> Result<Bytes, ProxmoxWorkerError> {
    Ok(Bytes::from(serde_json::to_vec(response)?))
}

/// Answers one lease operation, or ignores it when this worker does not hold the lease.
pub(crate) async fn handle(
    client: &Client,
    provider: &Arc<ProxmoxProvider>,
    message: Message,
) -> Result<(), ProxmoxWorkerError> {
    let tokens: Vec<&str> = message.subject.split('.').collect();
    let (verb, lease) = tokens.get(3).zip(tokens.get(4)).ok_or(ProxmoxWorkerError::Subject)?;
    let lease = LeaseId::new(*lease);
    if !provider.holds_lease(&lease).await || matches!(*verb, "cancel" | "purge") {
        return Ok(());
    }
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(client, &message).await?;
    let head: LeaseRef = decode(&payload)?;
    if head.lease_id != lease {
        return Err(ProxmoxWorkerError::LeaseMismatch);
    }
    let response = respond_to(client, provider, &lease, verb, &payload).await?;
    nats_transfer::respond(client, reply.as_str(), response).await?;
    Ok(())
}

async fn respond_to(
    client: &Client,
    provider: &ProxmoxProvider,
    lease: &LeaseId,
    verb: &str,
    payload: &[u8],
) -> Result<Bytes, ProxmoxWorkerError> {
    match verb {
        "run_command" => {
            let request: RunCommandRequest = decode(payload)?;
            let reply = match provider.run_command(&request).await {
                Ok(output) => {
                    fit_run_command_response(RunCommandResponse::Completed(output), client.server_info().max_payload)
                }
                Err(error) => RunCommandResponse::Denied(denial(lease, &error)),
            };
            encode(&reply)
        }
        "write_files" => {
            let request: WriteFilesRequest = decode(payload)?;
            encode(&match provider.write_files(&request).await {
                Ok(()) => WriteFilesResponse::Written,
                Err(error) => WriteFilesResponse::Denied(denial(lease, &error)),
            })
        }
        "read_file" => {
            let request: ReadFileRequest = decode(payload)?;
            let result = provider.read_file(&request).await;
            encode(&read_file_response(result, |error| denial(lease, &error)))
        }
        "file_op" => {
            let request: FileOpRequest = decode(payload)?;
            encode(&unsupported_file_op_response("proxmox", &request.op))
        }
        other => encode(&DeniedBody {
            outcome: "denied",
            kind: "invalid_spec",
            reason: format!("{other} is unavailable on the Proxmox worker"),
        }),
    }
}
