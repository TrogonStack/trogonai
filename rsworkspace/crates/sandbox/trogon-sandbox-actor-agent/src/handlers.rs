use std::future::Future;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    KillCommandRequest, KillCommandResponse, LeaseError, LeaseId, ModifyGroupMembershipRequest,
    ModifyGroupMembershipResponse, PollCommandRequest, PollCommandResponse, ReadFileRequest, ReadFileResponse,
    RunCommandRequest, RunCommandResponse, TerminalRequest, TerminalResponse, WriteFilesRequest, WriteFilesResponse,
    WriteStdinRequest, WriteStdinResponse,
};

use crate::ndjson::{encode_line, ndjson_stream, single_frame};
use crate::seed_receipt::apply_seed;
use crate::{
    ActorIdentity, AgentState, BindLeaseRequest, BindLeaseResponse, ExecFrame, LeaseBinding, TerminalFrame,
    create_group, create_user, modify_group_membership, read_file, write_files,
};

type Shared = State<Arc<AgentState>>;

/// Runs `run` for the lease the actor serves, or answers with the refusal built by `denied`.
///
/// A mismatch is a refusal rather than a best-effort execution: an actor is reused across a suspend and resume cycle,
/// so a request carrying a stale lease is a request whose sandbox no longer exists.
async fn for_lease<R, F, Fut>(state: &AgentState, lease_id: &LeaseId, denied: fn(LeaseError) -> R, run: F) -> Json<R>
where
    F: FnOnce(LeaseBinding) -> Fut,
    Fut: Future<Output = R>,
{
    match state.binding.resolve(lease_id).await {
        Ok(binding) => Json(run(binding).await),
        Err(error) => Json(denied(error.into())),
    }
}

pub(crate) async fn readyz() -> StatusCode {
    StatusCode::OK
}

pub(crate) async fn bind(State(state): Shared, Json(request): Json<BindLeaseRequest>) -> Response {
    let applied = request
        .entropy_seed
        .as_ref()
        .map(|seed| apply_seed(state.entropy_reseeder.as_ref(), seed))
        .transpose();
    let receipt = match applied {
        Ok(receipt) => receipt,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response(),
    };
    let lease_id = request.lease_id.clone();
    state.binding.bind(LeaseBinding::from(request)).await;
    match receipt {
        Some(entropy_reseed) => Json(BindLeaseResponse {
            lease_id,
            entropy_reseed,
        })
        .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

pub(crate) async fn exec(State(state): Shared, Json(request): Json<RunCommandRequest>) -> Response {
    match state.binding.resolve(&request.lease_id).await {
        Ok(binding) => {
            let frames = state.executions.run(request, binding).await;
            ndjson_stream(ReceiverStream::new(frames).map(|frame| Ok(encode_line(&frame))))
        }
        Err(error) => single_frame(&ExecFrame::Response(RunCommandResponse::Denied(error.into()))),
    }
}

pub(crate) async fn exec_poll(
    State(state): Shared,
    Json(request): Json<PollCommandRequest>,
) -> Json<PollCommandResponse> {
    let run = |_| state.executions.poll(&request.execution_id);
    for_lease(&state, &request.lease_id, PollCommandResponse::Denied, run).await
}

pub(crate) async fn exec_kill(
    State(state): Shared,
    Json(request): Json<KillCommandRequest>,
) -> Json<KillCommandResponse> {
    let run = |_| state.executions.kill(&request.execution_id, request.signal);
    for_lease(&state, &request.lease_id, KillCommandResponse::Denied, run).await
}

pub(crate) async fn exec_write_stdin(
    State(state): Shared,
    Json(request): Json<WriteStdinRequest>,
) -> Json<WriteStdinResponse> {
    let run = |_| state.executions.write_stdin(&request);
    for_lease(&state, request.lease_id(), WriteStdinResponse::Denied, run).await
}

pub(crate) async fn files_write(
    State(state): Shared,
    Json(request): Json<WriteFilesRequest>,
) -> Json<WriteFilesResponse> {
    let request = &request;
    let run = |binding| async move { write_files(request, &binding).await };
    for_lease(&state, &request.lease_id, WriteFilesResponse::Denied, run).await
}

pub(crate) async fn files_read(State(state): Shared, Json(request): Json<ReadFileRequest>) -> Json<ReadFileResponse> {
    let request = &request;
    let run = |binding| async move { read_file(request, &binding).await };
    for_lease(&state, &request.lease_id, ReadFileResponse::Denied, run).await
}

pub(crate) async fn users_create(
    State(state): Shared,
    Json(request): Json<CreateSandboxUserRequest>,
) -> Json<CreateSandboxUserResponse> {
    let run = |_| create_user(&request);
    for_lease(&state, &request.lease_id, CreateSandboxUserResponse::Denied, run).await
}

pub(crate) async fn groups_create(
    State(state): Shared,
    Json(request): Json<CreateSandboxGroupRequest>,
) -> Json<CreateSandboxGroupResponse> {
    let run = |_| create_group(&request);
    for_lease(&state, &request.lease_id, CreateSandboxGroupResponse::Denied, run).await
}

pub(crate) async fn group_membership(
    State(state): Shared,
    Json(request): Json<ModifyGroupMembershipRequest>,
) -> Json<ModifyGroupMembershipResponse> {
    let run = |_| modify_group_membership(&request);
    for_lease(&state, &request.lease_id, ModifyGroupMembershipResponse::Denied, run).await
}

/// Opens a terminal and streams its frames until it ends.
///
/// The connection is the terminal's lifetime: when the host drops it, the body is dropped, the frame receiver closes
/// and the session hangs its shell up.
pub(crate) async fn terminal_open(State(state): Shared, Json(request): Json<TerminalRequest>) -> Response {
    let binding = state.binding.resolve(&request.lease_id).await;
    let opened = binding
        .map_err(|error| TerminalResponse::Denied(error.into()))
        .and_then(|binding| {
            state
                .terminals
                .open(&binding, &request)
                .map_err(TerminalResponse::Failed)
        });
    match opened {
        Ok(frames) => {
            let terminal_id = request.terminal_id.clone();
            let first = TerminalFrame::Response(TerminalResponse::Opened { terminal_id });
            let output = ReceiverStream::new(frames).map(TerminalFrame::Output);
            ndjson_stream(
                futures::stream::once(async move { first })
                    .chain(output)
                    .map(|frame| Ok(encode_line(&frame))),
            )
        }
        Err(refusal) => single_frame(&TerminalFrame::Response(refusal)),
    }
}

pub(crate) async fn terminal_op(State(state): Shared, Json(request): Json<TerminalRequest>) -> Json<TerminalResponse> {
    let run = |_| state.terminals.apply(&request);
    for_lease(&state, &request.lease_id, TerminalResponse::Denied, run).await
}

pub(crate) async fn identity(State(state): Shared) -> Response {
    match state.identity.read().await {
        Ok(actor_id) => Json(ActorIdentity { actor_id }).into_response(),
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response(),
    }
}

/// Everything outside the agent's own paths goes to the workload the lease exposes.
pub(crate) async fn proxy(State(state): Shared, request: Request) -> Response {
    match state.binding.exposed_port().await {
        Some(port) => state.proxy.forward(port, request).await,
        None => (
            StatusCode::NOT_FOUND,
            "this sandbox exposes no port; only the reserved agent surface is served",
        )
            .into_response(),
    }
}
