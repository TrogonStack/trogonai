use serde_json::Value;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, KillCommandRequest, ListProcessesRequest, PollCommandRequest, ReadLogsRequest,
    RestartCommandRequest, RunCommandRequest, RunCommandResponse, RunCommandStartedResponse, StopCommandRequest,
    WaitReadyRequest, WaitSandboxReadyRequest, WriteStdinRequest,
};

use crate::command_payloads::kill_process;
use crate::encode::encode;
use crate::follow_logs_stream_id::follow_logs_stream_id;
use crate::from_payload::from_payload;
use crate::response_bodies::FollowingBody;
use crate::{DispatchError, RunCommandOutcome, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_run_command" => run_command(ctx, payload).await?,
        "sandbox_poll_command" => {
            let request: PollCommandRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .poll_command(ctx.api_key(), &request.lease_id, &request.execution_id)
                .await?;
            encode(response)?
        }
        "sandbox_list_processes" => {
            let request: ListProcessesRequest = from_payload(payload)?;
            encode(ctx.backend.list_processes(ctx.api_key(), &request.lease_id).await?)?
        }
        "sandbox_wait_ready" => {
            let request: WaitReadyRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .wait_ready(
                    ctx.api_key(),
                    &request.lease_id,
                    &request.execution_id,
                    request.timeout_seconds,
                )
                .await?;
            encode(response)?
        }
        "sandbox_wait_sandbox_ready" => {
            let request: WaitSandboxReadyRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .wait_sandbox_ready(ctx.api_key(), &request.lease_id, request.ports, request.timeout_seconds)
                .await?;
            encode(response)?
        }
        "sandbox_kill_process" => {
            let (lease_id, name, signal) = kill_process(payload)?;
            let response = ctx
                .backend
                .kill_process(ctx.api_key(), &lease_id, &name, signal)
                .await?;
            encode(response)?
        }
        "sandbox_cancel_command_launch" => {
            let request: CancelCommandLaunchRequest = from_payload(payload)?;
            encode(ctx.backend.cancel_command_launch(ctx.api_key(), request).await?)?
        }
        "sandbox_kill_command" => {
            let request: KillCommandRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .kill_command(ctx.api_key(), &request.lease_id, &request.execution_id, request.signal)
                .await?;
            encode(response)?
        }
        "sandbox_stop_command" => {
            let request: StopCommandRequest = from_payload(payload)?;
            encode(ctx.backend.stop_command(ctx.api_key(), request).await?)?
        }
        "sandbox_restart_command" => {
            let request: RestartCommandRequest = from_payload(payload)?;
            encode(ctx.backend.restart_command(ctx.api_key(), request).await?)?
        }
        "sandbox_read_logs" => {
            let request: ReadLogsRequest = from_payload(payload)?;
            encode(ctx.backend.read_logs(ctx.api_key(), request).await?)?
        }
        "sandbox_follow_logs" => follow_logs(ctx, payload).await?,
        "sandbox_write_stdin" => {
            let request: WriteStdinRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .write_stdin(
                    ctx.api_key(),
                    request.lease_id(),
                    request.execution_id(),
                    request.seq(),
                    request.data().clone(),
                    request.eof(),
                )
                .await?;
            encode(response)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn run_command(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let request: RunCommandRequest = from_payload(payload)?;
    let outcome = ctx.backend.run_command(ctx.api_key(), request).await?;
    let response = match outcome {
        RunCommandOutcome::Completed(output) => RunCommandResponse::Completed(output),
        RunCommandOutcome::Started { execution_id, origin } => {
            RunCommandResponse::Started(RunCommandStartedResponse { execution_id, origin })
        }
    };
    encode(response)
}

async fn follow_logs(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let request: ReadLogsRequest = from_payload(payload)?;
    let stream_id = follow_logs_stream_id(&request.lease_id, &request.execution_id);
    ctx.backend
        .follow_logs(ctx.session.api_key.clone(), request, ctx.events.clone())
        .await?;
    encode(FollowingBody {
        following: true,
        stream_id,
    })
}
