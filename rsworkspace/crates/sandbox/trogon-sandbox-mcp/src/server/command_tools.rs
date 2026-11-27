use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ProgressToken, RequestMetaObject};
use rmcp::{Peer, RoleServer, tool, tool_router};
use trogon_sandbox_contract::{ProcessName, StdinChunk, StdinEof, StdinSeq};

use super::SandboxMcpServer;
use super::command_output::{
    attached_output, await_completed, completed_output, logs_output, poll_output, started_execution_id,
};
use crate::inputs::command_inputs::{
    KillCommandInput, KillProcessInput, ListProcessesInput, PollCommandInput, PollCommandOutput, ReadLogsInput,
    ReadLogsOutput, RestartCommandInput, RunCommandInput, RunCommandOutput, StopCommandInput, WaitReadyInput,
    WriteStdinInput,
};
use crate::launch::{LaunchMode, plan};
use crate::parse;
use crate::progress::pump_progress;
use crate::reply::{JsonReply, json_reply};
use crate::tool_error::ToolError;

#[tool_router(router = command_router, vis = "pub(super)")]
impl SandboxMcpServer {
    #[tool(
        description = "Run a command inside a running lease's sandbox. Mirrors trogon_sandbox_contract::RunCommandRequest. Set detached=true to return immediately with an execution_id pollable via sandbox_poll_command. Set stream_progress=true (with a progressToken on the call) to receive the output as MCP progress notifications while it runs; the result is unchanged. Set redact_values to scrub exact literal secrets from the output (encoded copies are not caught). With detached=true and stdin=interactive, the command's stdin stays open for sandbox_write_stdin."
    )]
    async fn sandbox_run_command(
        &self,
        peer: Peer<RoleServer>,
        meta: RequestMetaObject,
        params: Parameters<RunCommandInput>,
    ) -> Result<Json<RunCommandOutput>, ToolError> {
        let progress = meta.get_progress_token().map(|token| (peer, token));
        self.run_command(progress, params.0).await.map(Json)
    }

    #[tool(
        description = "Poll the status of a previously started detached command. Mirrors trogon_sandbox_contract::PollCommandRequest."
    )]
    async fn sandbox_poll_command(
        &self,
        params: Parameters<PollCommandInput>,
    ) -> Result<Json<PollCommandOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        let response = self.client()?.poll_command(&lease_id, &execution_id).await?;
        poll_output(response).map(Json)
    }

    #[tool(
        description = "List the named or probed processes started in a lease with sandbox_run_command's process field, with their state. Replies unsupported when the provider does not declare supports_named_processes."
    )]
    async fn sandbox_list_processes(&self, params: Parameters<ListProcessesInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        json_reply(&self.client()?.list_processes(&lease_id).await?)
    }

    #[tool(
        description = "Wait for the readiness probe of a process started with sandbox_run_command's process field. Replies ready, not_ready (timeout), failed (the process exited first), no_probe, not_found or unsupported."
    )]
    async fn sandbox_wait_ready(&self, params: Parameters<WaitReadyInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        let wait = params.0.timeout_seconds.unwrap_or(30);
        json_reply(&self.client()?.wait_ready(&lease_id, &execution_id, wait).await?)
    }

    #[tool(
        description = "Send a Unix signal to the running process with this name in a lease. Replies not_found when no running process has the name, so repeating the call is safe."
    )]
    async fn sandbox_kill_process(&self, params: Parameters<KillProcessInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let signal = parse::signal(params.0.signal)?;
        let name = ProcessName::new(params.0.name).map_err(|e| ToolError::invalid("name", e))?;
        json_reply(&self.client()?.kill_process_by_name(&lease_id, &name, signal).await?)
    }

    #[tool(
        description = "Send a Unix signal to a previously started detached command. Mirrors trogon_sandbox_contract::KillCommandRequest."
    )]
    async fn sandbox_kill_command(&self, params: Parameters<KillCommandInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        let signal = parse::signal(params.0.signal)?;
        json_reply(&self.client()?.kill_command(&lease_id, &execution_id, signal).await?)
    }

    #[tool(
        description = "Gracefully stop a detached command: SIGTERM, wait up to grace_seconds, then SIGKILL. Mirrors trogon_sandbox_contract::StopCommandRequest. Providers without graceful stop reply unsupported."
    )]
    async fn sandbox_stop_command(&self, params: Parameters<StopCommandInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        json_reply(
            &self
                .client()?
                .stop_command(&lease_id, &execution_id, params.0.grace_seconds)
                .await?,
        )
    }

    #[tool(
        description = "Stop a detached command and start it again with the same process spec under a new execution_id. Mirrors trogon_sandbox_contract::RestartCommandRequest. Providers without restart reply unsupported."
    )]
    async fn sandbox_restart_command(&self, params: Parameters<RestartCommandInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        json_reply(
            &self
                .client()?
                .restart_command(&lease_id, &execution_id, params.0.grace_seconds)
                .await?,
        )
    }

    #[tool(
        description = "Read one page of a command's durable output after a cursor. Mirrors trogon_sandbox_contract::ReadLogsRequest. Resume with next_seq while has_more is true; truncated means retention dropped the start of the output. Unsupported when the broker keeps no durable logs."
    )]
    async fn sandbox_read_logs(&self, params: Parameters<ReadLogsInput>) -> Result<Json<ReadLogsOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        let response = self
            .client()?
            .read_logs(&lease_id, &execution_id, params.0.after_seq, params.0.max_bytes)
            .await?;
        logs_output(response).map(Json)
    }

    #[tool(
        description = "Write to the stdin of a detached command started with stdin=interactive, and close it with eof=true. Mirrors trogon_sandbox_contract::WriteStdinRequest. On buffered, backpressure, or out_of_window, resend from next_seq."
    )]
    async fn sandbox_write_stdin(&self, params: Parameters<WriteStdinInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let execution_id = parse::execution_id(params.0.execution_id);
        let data = STANDARD
            .decode(params.0.data_base64.as_bytes())
            .map_err(|e| ToolError::invalid("data_base64", e))?;
        let eof = StdinEof::from(params.0.eof);
        if data.is_empty() && !eof.closes() {
            return Err(ToolError::InvalidCombination {
                option: "an empty data_base64",
                requires: "eof=true",
            });
        }
        let chunk = StdinChunk::new(data).map_err(|e| ToolError::invalid("data_base64", e))?;
        let seq = StdinSeq::new(params.0.seq);
        json_reply(
            &self
                .client()?
                .write_stdin(&lease_id, &execution_id, seq, chunk, eof)
                .await?,
        )
    }
}

impl SandboxMcpServer {
    /// The body of `sandbox_run_command`. `progress` is the peer and token to report output to when the caller
    /// asked for progress notifications.
    async fn run_command(
        &self,
        progress: Option<(Peer<RoleServer>, ProgressToken)>,
        input: RunCommandInput,
    ) -> Result<RunCommandOutput, ToolError> {
        self.state.require_auth()?;
        self.state
            .policy()
            .commands
            .check(&input.command)
            .map_err(ToolError::CommandDenied)?;
        let launch = plan(input, progress.is_some())?;
        let client = self.client()?;
        let lease_id = launch.request.lease_id.clone();
        let timeout_seconds = launch.request.timeout_seconds;
        let response = client.send_run_command(launch.request).await?;
        match (launch.mode, progress) {
            (LaunchMode::Detached, _) => {
                let execution_id = started_execution_id(response)?;
                Ok(RunCommandOutput::Started {
                    execution_id: execution_id.to_string(),
                })
            }
            (LaunchMode::Progress, Some((peer, token))) => {
                let execution_id = started_execution_id(response)?;
                let events = client.follow_logs(&lease_id, execution_id.clone(), 0);
                pump_progress(events, &token, |note| {
                    let peer = peer.clone();
                    async move { peer.notify_progress(note).await.is_ok() }
                })
                .await;
                let output = await_completed(client, &lease_id, &execution_id, timeout_seconds).await?;
                Ok(completed_output(&output))
            }
            _ => Ok(completed_output(&attached_output(response)?)),
        }
    }
}
