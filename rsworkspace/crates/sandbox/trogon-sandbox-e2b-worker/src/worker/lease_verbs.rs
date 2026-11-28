use std::sync::Arc;

use async_nats::Client;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CheckpointRunningRequest, CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest,
    CreateSandboxUserResponse, FileOpRequest, GetSandboxDomainRequest, KillCommandRequest, LeaseError,
    ListProcessesRequest, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, NativeHandleRequest,
    NetworkPolicyUpdateRequest, NetworkPolicyUpdateResponse, PollCommandRequest, ReadFileRequest, SandboxFreezeRequest,
    SandboxFreezeResponse, SandboxSuspendRequest, SandboxSuspendResponse, SandboxWakeRequest, SandboxWakeResponse,
    SnapshotExportError, SnapshotExportOutcome, SuspendLevel, WriteFilesRequest, WriteStdinRequest,
};
use trogon_sandbox_protocol::{encode_read_file_response_within, fit_poll_command_response, nats_transfer};

use crate::worker::E2bWorkerError;
use crate::worker::constants as subjects;
use crate::worker::respond_if_held::serve;
use crate::{E2bWorkerProvider, LeaseSandbox};

type Provider = Arc<E2bWorkerProvider>;
type Reply = Result<Vec<u8>, E2bWorkerError>;

fn encode<T: Serialize>(response: &T) -> Reply {
    Ok(serde_json::to_vec(response)?)
}

fn unsupported(what: &str) -> LeaseError {
    LeaseError::Backend {
        message: format!("{what} are not supported on the E2B provider"),
    }
}

async fn poll_command(e2b: Provider, sandbox: LeaseSandbox, request: PollCommandRequest, max: usize) -> Reply {
    let reply = e2b
        .exec()
        .handle_poll_command(&sandbox, request.lease_id.as_str(), request.execution_id.as_str())
        .await;
    encode(&fit_poll_command_response(reply, max))
}

async fn list_processes(e2b: Provider, sandbox: LeaseSandbox, request: ListProcessesRequest, _max: usize) -> Reply {
    encode(
        &e2b.exec()
            .handle_list_processes(&sandbox, request.lease_id.as_str())
            .await,
    )
}

async fn kill_command(e2b: Provider, sandbox: LeaseSandbox, request: KillCommandRequest, _max: usize) -> Reply {
    encode(
        &e2b.exec()
            .handle_kill_command(&sandbox, request.lease_id.as_str(), &request)
            .await,
    )
}

async fn write_stdin(e2b: Provider, _sandbox: LeaseSandbox, _request: WriteStdinRequest, _max: usize) -> Reply {
    encode(&e2b.exec().handle_write_stdin())
}

async fn write_files(e2b: Provider, sandbox: LeaseSandbox, request: WriteFilesRequest, _max: usize) -> Reply {
    encode(&e2b.exec().handle_write_files(&sandbox, &request).await)
}

async fn read_file(e2b: Provider, sandbox: LeaseSandbox, request: ReadFileRequest, _max: usize) -> Reply {
    let response = e2b.exec().handle_read_file(&sandbox, &request).await;
    Ok(encode_read_file_response_within(
        &response,
        &request.path,
        nats_transfer::MAX_TRANSFER_BYTES,
    )?)
}

async fn file_op(e2b: Provider, sandbox: LeaseSandbox, request: FileOpRequest, _max: usize) -> Reply {
    encode(&e2b.exec().handle_file_op(&sandbox, &request).await)
}

async fn get_domain(e2b: Provider, sandbox: LeaseSandbox, request: GetSandboxDomainRequest, _max: usize) -> Reply {
    encode(
        &e2b.get_domain_for_lease(&request.lease_id, &sandbox, request.port)
            .await,
    )
}

async fn pause(e2b: Provider, _sandbox: LeaseSandbox, request: SandboxFreezeRequest, _max: usize) -> Reply {
    encode(&e2b.set_paused_for_lease(&request.lease_id, true).await)
}

async fn resume(e2b: Provider, _sandbox: LeaseSandbox, request: SandboxFreezeRequest, _max: usize) -> Reply {
    encode(&e2b.set_paused_for_lease(&request.lease_id, false).await)
}

async fn suspend(e2b: Provider, _sandbox: LeaseSandbox, request: SandboxSuspendRequest, _max: usize) -> Reply {
    if request.level != SuspendLevel::Memory {
        return encode(&SandboxSuspendResponse::Unsupported);
    }
    encode(&suspended(e2b.set_paused_for_lease(&request.lease_id, true).await))
}

fn suspended(freeze: SandboxFreezeResponse) -> SandboxSuspendResponse {
    match freeze {
        SandboxFreezeResponse::Done => SandboxSuspendResponse::Done {
            artifact: None,
            artifact_bytes: None,
        },
        SandboxFreezeResponse::Unsupported => SandboxSuspendResponse::Unsupported,
        SandboxFreezeResponse::Denied(error) => SandboxSuspendResponse::Denied(error),
    }
}

async fn wake(e2b: Provider, _sandbox: LeaseSandbox, request: SandboxWakeRequest, _max: usize) -> Reply {
    if request.level != SuspendLevel::Memory {
        return encode(&SandboxWakeResponse::Unsupported);
    }
    encode(&woken(e2b.set_paused_for_lease(&request.lease_id, false).await))
}

fn woken(freeze: SandboxFreezeResponse) -> SandboxWakeResponse {
    match freeze {
        SandboxFreezeResponse::Done => SandboxWakeResponse::Done,
        SandboxFreezeResponse::Unsupported => SandboxWakeResponse::Unsupported,
        SandboxFreezeResponse::Denied(error) => SandboxWakeResponse::Denied(error),
    }
}

async fn native_handle(e2b: Provider, _sandbox: LeaseSandbox, request: NativeHandleRequest, _max: usize) -> Reply {
    encode(&e2b.native_handle_for_lease(&request.lease_id).await)
}

async fn checkpoint_running(
    e2b: Provider,
    _sandbox: LeaseSandbox,
    request: CheckpointRunningRequest,
    _max: usize,
) -> Reply {
    match request.native_name {
        Some(name) => encode(&e2b.snapshot_lease(&request.lease_id, &name).await),
        None => encode(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
            message: subjects::CHECKPOINT_NEEDS_NATIVE.to_owned(),
        })),
    }
}

async fn network_policy_update(
    _e2b: Provider,
    _sandbox: LeaseSandbox,
    _request: NetworkPolicyUpdateRequest,
    _max: usize,
) -> Reply {
    encode(&NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy {
        reason: subjects::NETWORK_POLICY_AT_CREATE.to_owned(),
    }))
}

async fn create_user(_e2b: Provider, _sandbox: LeaseSandbox, _request: CreateSandboxUserRequest, _max: usize) -> Reply {
    encode(&CreateSandboxUserResponse::Denied(unsupported("sandbox users")))
}

async fn create_group(
    _e2b: Provider,
    _sandbox: LeaseSandbox,
    _request: CreateSandboxGroupRequest,
    _max: usize,
) -> Reply {
    encode(&CreateSandboxGroupResponse::Denied(unsupported("sandbox groups")))
}

async fn modify_group_membership(
    _e2b: Provider,
    _sandbox: LeaseSandbox,
    _request: ModifyGroupMembershipRequest,
    _max: usize,
) -> Reply {
    encode(&ModifyGroupMembershipResponse::Denied(unsupported(
        "sandbox group membership",
    )))
}

/// Subscribes the per-lease verbs the broker routes to this provider. Each is broadcast to every worker, and only the
/// worker holding the lease replies.
pub(crate) async fn serve_lease_verbs(
    client: &Client,
    e2b: &Provider,
    cancel: &CancellationToken,
) -> Result<(), E2bWorkerError> {
    serve(client, e2b, cancel, subjects::POLL_COMMAND, false, poll_command).await?;
    serve(client, e2b, cancel, subjects::LIST_PROCESSES, false, list_processes).await?;
    serve(client, e2b, cancel, subjects::KILL_COMMAND, false, kill_command).await?;
    serve(client, e2b, cancel, subjects::WRITE_STDIN, false, write_stdin).await?;
    serve(client, e2b, cancel, subjects::WRITE_FILES, true, write_files).await?;
    serve(client, e2b, cancel, subjects::READ_FILE, true, read_file).await?;
    serve(client, e2b, cancel, subjects::FILE_OP, true, file_op).await?;
    serve(client, e2b, cancel, subjects::GET_DOMAIN, false, get_domain).await?;
    serve(client, e2b, cancel, subjects::PAUSE, false, pause).await?;
    serve(client, e2b, cancel, subjects::RESUME, false, resume).await?;
    serve(client, e2b, cancel, subjects::SUSPEND, false, suspend).await?;
    serve(client, e2b, cancel, subjects::WAKE, false, wake).await?;
    serve(client, e2b, cancel, subjects::NATIVE_HANDLE, false, native_handle).await?;
    serve(
        client,
        e2b,
        cancel,
        subjects::CHECKPOINT_RUNNING,
        false,
        checkpoint_running,
    )
    .await?;
    serve(
        client,
        e2b,
        cancel,
        subjects::NETWORK_POLICY_UPDATE,
        false,
        network_policy_update,
    )
    .await?;
    serve(client, e2b, cancel, subjects::CREATE_USER, false, create_user).await?;
    serve(client, e2b, cancel, subjects::CREATE_GROUP, false, create_group).await?;
    serve(
        client,
        e2b,
        cancel,
        subjects::MODIFY_GROUP_MEMBERSHIP,
        false,
        modify_group_membership,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
