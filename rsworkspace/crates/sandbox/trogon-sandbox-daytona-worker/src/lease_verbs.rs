use std::sync::Arc;

use async_nats::Client;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CheckpointRunningRequest, CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest,
    CreateSandboxUserResponse, FileOpRequest, GetSandboxDomainRequest, KillCommandRequest, LeaseError,
    ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, NativeHandleRequest, NetworkPolicyUpdateRequest,
    NetworkPolicyUpdateResponse, PollCommandRequest, ReadFileRequest, RunCommandRequest, SandboxFreezeRequest,
    SandboxFreezeResponse, SandboxSuspendRequest, SandboxSuspendResponse, SandboxWakeRequest, SandboxWakeResponse,
    SnapshotExportError, SnapshotExportOutcome, SshAccessRequest, SuspendLevel, WriteFilesRequest, WriteStdinRequest,
};
use trogon_sandbox_protocol::{
    encode_read_file_response_within, fit_poll_command_response, fit_run_command_response, nats_transfer,
    unsupported_file_op_response,
};

use crate::constants::{self as subjects, UNSUPPORTED};
use crate::respond_if_held::serve;
use crate::{DaytonaWorkerError, DaytonaWorkerProvider, LeaseSandbox};

type Provider = Arc<DaytonaWorkerProvider>;
type Reply = Result<Vec<u8>, DaytonaWorkerError>;

fn encode<T: Serialize>(response: &T) -> Reply {
    Ok(serde_json::to_vec(response)?)
}

fn unsupported(what: &str) -> LeaseError {
    LeaseError::Backend {
        message: format!("{what} are not supported on the Daytona provider"),
    }
}

async fn run_command(daytona: Provider, sandbox: LeaseSandbox, request: RunCommandRequest, max: usize) -> Reply {
    encode(&fit_run_command_response(
        daytona.exec().handle_run_command(&sandbox, &request).await,
        max,
    ))
}

async fn poll_command(daytona: Provider, sandbox: LeaseSandbox, request: PollCommandRequest, max: usize) -> Reply {
    let reply = daytona
        .exec()
        .handle_poll_command(&sandbox, request.lease_id.as_str(), request.execution_id.as_str())
        .await;
    encode(&fit_poll_command_response(reply, max))
}

async fn kill_command(daytona: Provider, sandbox: LeaseSandbox, request: KillCommandRequest, _max: usize) -> Reply {
    encode(
        &daytona
            .exec()
            .handle_kill_command(&sandbox, request.lease_id.as_str(), &request)
            .await,
    )
}

async fn write_stdin(daytona: Provider, sandbox: LeaseSandbox, request: WriteStdinRequest, _max: usize) -> Reply {
    encode(&daytona.exec().handle_write_stdin(&sandbox, &request).await)
}

async fn write_files(daytona: Provider, sandbox: LeaseSandbox, request: WriteFilesRequest, _max: usize) -> Reply {
    encode(&daytona.exec().handle_write_files(&sandbox, &request).await)
}

async fn read_file(daytona: Provider, sandbox: LeaseSandbox, request: ReadFileRequest, _max: usize) -> Reply {
    let response = daytona.exec().handle_read_file(&sandbox, &request).await;
    let limit = nats_transfer::MAX_TRANSFER_BYTES;
    Ok(encode_read_file_response_within(&response, &request.path, limit)?)
}

async fn file_op(_daytona: Provider, _sandbox: LeaseSandbox, request: FileOpRequest, _max: usize) -> Reply {
    encode(&unsupported_file_op_response("daytona", &request.op))
}

async fn get_domain(daytona: Provider, sandbox: LeaseSandbox, request: GetSandboxDomainRequest, _max: usize) -> Reply {
    encode(&daytona.exec().handle_get_domain(&sandbox, request.port).await)
}

async fn pause(daytona: Provider, _sandbox: LeaseSandbox, request: SandboxFreezeRequest, _max: usize) -> Reply {
    encode(&daytona.set_paused_for_lease(&request.lease_id, true).await)
}

async fn resume(daytona: Provider, _sandbox: LeaseSandbox, request: SandboxFreezeRequest, _max: usize) -> Reply {
    encode(&daytona.set_paused_for_lease(&request.lease_id, false).await)
}

async fn suspend(daytona: Provider, _sandbox: LeaseSandbox, request: SandboxSuspendRequest, _max: usize) -> Reply {
    if request.level != SuspendLevel::Disk {
        return encode(&SandboxSuspendResponse::Unsupported);
    }
    encode(&suspended(daytona.set_paused_for_lease(&request.lease_id, true).await))
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

async fn wake(daytona: Provider, _sandbox: LeaseSandbox, request: SandboxWakeRequest, _max: usize) -> Reply {
    if request.level != SuspendLevel::Disk {
        return encode(&SandboxWakeResponse::Unsupported);
    }
    encode(&woken(daytona.set_paused_for_lease(&request.lease_id, false).await))
}

fn woken(freeze: SandboxFreezeResponse) -> SandboxWakeResponse {
    match freeze {
        SandboxFreezeResponse::Done => SandboxWakeResponse::Done,
        SandboxFreezeResponse::Unsupported => SandboxWakeResponse::Unsupported,
        SandboxFreezeResponse::Denied(error) => SandboxWakeResponse::Denied(error),
    }
}

async fn native_handle(daytona: Provider, _sandbox: LeaseSandbox, request: NativeHandleRequest, _max: usize) -> Reply {
    encode(&daytona.native_handle_for_lease(&request.lease_id).await)
}

async fn ssh_access(daytona: Provider, sandbox: LeaseSandbox, request: SshAccessRequest, _max: usize) -> Reply {
    encode(&daytona.ssh_access_for_sandbox(&sandbox.id, request.ttl_seconds).await)
}

async fn checkpoint_running(
    daytona: Provider,
    sandbox: LeaseSandbox,
    request: CheckpointRunningRequest,
    _max: usize,
) -> Reply {
    match request.native_name.as_deref() {
        Some(name) => encode(&daytona.native_snapshot(&sandbox.id, name).await),
        None => encode(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
            message: UNSUPPORTED.to_owned(),
        })),
    }
}

async fn network_policy_update(
    _daytona: Provider,
    _sandbox: LeaseSandbox,
    _request: NetworkPolicyUpdateRequest,
    _max: usize,
) -> Reply {
    encode(&NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy {
        reason: "this worker applies network settings at sandbox create time only".to_owned(),
    }))
}

async fn create_user(
    _daytona: Provider,
    _sandbox: LeaseSandbox,
    _request: CreateSandboxUserRequest,
    _max: usize,
) -> Reply {
    encode(&CreateSandboxUserResponse::Denied(unsupported("sandbox users")))
}

async fn create_group(
    _daytona: Provider,
    _sandbox: LeaseSandbox,
    _request: CreateSandboxGroupRequest,
    _max: usize,
) -> Reply {
    encode(&CreateSandboxGroupResponse::Denied(unsupported("sandbox groups")))
}

async fn modify_group_membership(
    _daytona: Provider,
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
    daytona: &Provider,
    cancel: &CancellationToken,
) -> Result<(), DaytonaWorkerError> {
    serve(client, daytona, cancel, subjects::RUN_COMMAND, false, run_command).await?;
    serve(client, daytona, cancel, subjects::POLL_COMMAND, false, poll_command).await?;
    serve(client, daytona, cancel, subjects::KILL_COMMAND, false, kill_command).await?;
    serve(client, daytona, cancel, subjects::WRITE_STDIN, false, write_stdin).await?;
    serve(client, daytona, cancel, subjects::WRITE_FILES, true, write_files).await?;
    serve(client, daytona, cancel, subjects::READ_FILE, true, read_file).await?;
    serve(client, daytona, cancel, subjects::FILE_OP, true, file_op).await?;
    serve(client, daytona, cancel, subjects::GET_DOMAIN, false, get_domain).await?;
    serve(client, daytona, cancel, subjects::PAUSE, false, pause).await?;
    serve(client, daytona, cancel, subjects::RESUME, false, resume).await?;
    serve(client, daytona, cancel, subjects::SUSPEND, false, suspend).await?;
    serve(client, daytona, cancel, subjects::WAKE, false, wake).await?;
    serve(client, daytona, cancel, subjects::NATIVE_HANDLE, false, native_handle).await?;
    serve(client, daytona, cancel, subjects::SSH_ACCESS, false, ssh_access).await?;
    serve(
        client,
        daytona,
        cancel,
        subjects::CHECKPOINT_RUNNING,
        false,
        checkpoint_running,
    )
    .await?;
    serve(
        client,
        daytona,
        cancel,
        subjects::NETWORK_POLICY_UPDATE,
        false,
        network_policy_update,
    )
    .await?;
    serve(client, daytona, cancel, subjects::CREATE_USER, false, create_user).await?;
    serve(client, daytona, cancel, subjects::CREATE_GROUP, false, create_group).await?;
    serve(
        client,
        daytona,
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
