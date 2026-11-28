use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use trogon_sandbox_contract::{
    CheckpointRunningRequest, FileOpRequest, GetSandboxDomainRequest, LeaseId, ReadFileRequest, RunCommandRequest,
    RunCommandResponse, SandboxFreezeResponse, SandboxSuspendRequest, SandboxSuspendResponse, SandboxWakeRequest,
    SandboxWakeResponse, SnapshotExportError, SnapshotExportOutcome, SuspendLevel, WriteFilesRequest,
    WriteFilesResponse,
};
use trogon_sandbox_protocol::{
    fit_run_command_response, nats_transfer, read_file_response, unsupported_file_op_response,
};

use crate::create_requests::DeniedBody;
use crate::provider_edge::denial;
use crate::{FlyProvider, FlyWorkerError};

#[derive(Deserialize)]
struct LeaseRef {
    lease_id: LeaseId,
}

fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FlyWorkerError> {
    Ok(serde_json::from_slice(payload)?)
}

fn encode<T: Serialize>(response: &T) -> Result<Bytes, FlyWorkerError> {
    Ok(Bytes::from(serde_json::to_vec(response)?))
}

fn unsupported_snapshot() -> SnapshotExportOutcome {
    SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
        message: "Fly has no Machine filesystem export; with a workspace volume it takes native volume snapshots, \
                  and suspend memory is offered as pause"
            .to_owned(),
    })
}

/// Answers one lease operation, or ignores it when this worker does not hold the lease.
pub(crate) async fn handle(
    client: &Client,
    provider: &Arc<FlyProvider>,
    message: Message,
) -> Result<(), FlyWorkerError> {
    let tokens: Vec<&str> = message.subject.split('.').collect();
    let (verb, lease) = tokens.get(3).zip(tokens.get(4)).ok_or(FlyWorkerError::Subject)?;
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
        return Err(FlyWorkerError::LeaseMismatch);
    }
    let response = respond_to(client, provider, &lease, verb, &payload).await?;
    nats_transfer::respond(client, reply.as_str(), response).await?;
    Ok(())
}

async fn respond_to(
    client: &Client,
    provider: &FlyProvider,
    lease: &LeaseId,
    verb: &str,
    payload: &[u8],
) -> Result<Bytes, FlyWorkerError> {
    match verb {
        "checkpoint_running" => {
            let request: CheckpointRunningRequest = decode(payload)?;
            // A lease released mid-request has no volume to snapshot, which reads as unsupported.
            let outcome = match request.native_name {
                Some(_) => provider
                    .native_snapshot(lease)
                    .await
                    .unwrap_or_else(unsupported_snapshot),
                None => unsupported_snapshot(),
            };
            encode(&outcome)
        }
        "snapshot" => encode(&unsupported_snapshot()),
        "get_domain" => {
            let request: GetSandboxDomainRequest = decode(payload)?;
            encode(&provider.get_domain(lease, request.port).await)
        }
        "pause" => encode(
            &provider
                .pause(lease)
                .await
                .map_or_else(SandboxFreezeResponse::Denied, |()| SandboxFreezeResponse::Done),
        ),
        "resume" => encode(
            &provider
                .thaw(lease)
                .await
                .map_or_else(SandboxFreezeResponse::Denied, |()| SandboxFreezeResponse::Done),
        ),
        // Fly offers memory suspend as pause, so the verbs share its machinery.
        "suspend" => {
            let request: SandboxSuspendRequest = decode(payload)?;
            encode(&match request.level {
                SuspendLevel::Memory => provider
                    .pause(lease)
                    .await
                    .map_or_else(SandboxSuspendResponse::Denied, |()| SandboxSuspendResponse::Done {
                        artifact: None,
                        artifact_bytes: None,
                    }),
                SuspendLevel::Disk => SandboxSuspendResponse::Unsupported,
            })
        }
        "wake" => {
            let request: SandboxWakeRequest = decode(payload)?;
            encode(&match request.level {
                SuspendLevel::Memory => provider
                    .thaw(lease)
                    .await
                    .map_or_else(SandboxWakeResponse::Denied, |()| SandboxWakeResponse::Done),
                SuspendLevel::Disk => SandboxWakeResponse::Unsupported,
            })
        }
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
            encode(&unsupported_file_op_response("fly", &request.op))
        }
        other => encode(&DeniedBody {
            outcome: "denied",
            kind: "invalid_spec",
            reason: format!("{other} is unavailable on the Fly worker"),
        }),
    }
}
