//! Pure mappings from the broker's command replies to tool outputs, and the wait a progress run ends with.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, LeaseId, OutputStream, PollCommandResponse, ReadLogsResponse, RunCommandResponse,
};

use crate::constants::{COMPLETION_GRACE_SECONDS, COMPLETION_POLL_INTERVAL};
use crate::inputs::command_inputs::{LogChunkOutput, PollCommandOutput, ReadLogsOutput, RunCommandOutput};
use crate::tool_error::ToolError;

pub(super) fn completed_output(output: &CommandOutput) -> RunCommandOutput {
    RunCommandOutput::Completed {
        exit_code: output.exit_code,
        stdout_base64: STANDARD.encode(&output.stdout),
        stderr_base64: STANDARD.encode(&output.stderr),
    }
}

/// The output of a foreground launch, or the broker's refusal.
pub(super) fn attached_output(response: RunCommandResponse) -> Result<CommandOutput, ToolError> {
    match response {
        RunCommandResponse::Completed(output) => Ok(output),
        RunCommandResponse::Started(_) => Err(ToolError::UnexpectedReply(
            "a started reply to a foreground run_command",
        )),
        RunCommandResponse::Denied(e) => Err(ToolError::denied(e)),
    }
}

/// The execution id of a detached launch, or the broker's refusal.
pub(super) fn started_execution_id(response: RunCommandResponse) -> Result<ExecutionId, ToolError> {
    match response {
        RunCommandResponse::Started(started) => Ok(started.execution_id),
        RunCommandResponse::Completed(_) => Err(ToolError::UnexpectedReply(
            "a completed reply to a detached run_command",
        )),
        RunCommandResponse::Denied(e) => Err(ToolError::denied(e)),
    }
}

/// A poll reply with the broker's refusal already raised.
pub(super) enum Polled {
    Running,
    Completed(CommandOutput),
    NotFound,
}

pub(super) fn polled(response: PollCommandResponse) -> Result<Polled, ToolError> {
    match response {
        PollCommandResponse::Running => Ok(Polled::Running),
        PollCommandResponse::Completed(output) => Ok(Polled::Completed(output)),
        PollCommandResponse::NotFound => Ok(Polled::NotFound),
        PollCommandResponse::Denied(e) => Err(ToolError::denied(e)),
    }
}

pub(super) fn poll_output(response: PollCommandResponse) -> Result<PollCommandOutput, ToolError> {
    Ok(match polled(response)? {
        Polled::Running => PollCommandOutput::Running,
        Polled::Completed(output) => PollCommandOutput::Completed {
            exit_code: output.exit_code,
            stdout_base64: STANDARD.encode(&output.stdout),
            stderr_base64: STANDARD.encode(&output.stderr),
        },
        Polled::NotFound => PollCommandOutput::NotFound,
    })
}

pub(super) fn logs_output(response: ReadLogsResponse) -> Result<ReadLogsOutput, ToolError> {
    match response {
        ReadLogsResponse::Logs {
            chunks,
            next_seq,
            has_more,
            truncated,
            ..
        } => Ok(ReadLogsOutput::Logs {
            chunks: chunks
                .into_iter()
                .map(|c| LogChunkOutput {
                    log_seq: c.log_seq,
                    stream: stream_name(c.stream).to_string(),
                    data_base64: STANDARD.encode(&c.data),
                })
                .collect(),
            next_seq,
            has_more: has_more.has_more(),
            truncated: truncated.is_truncated(),
        }),
        ReadLogsResponse::Unsupported => Ok(ReadLogsOutput::Unsupported),
        ReadLogsResponse::Denied(e) => Err(ToolError::denied(e)),
    }
}

fn stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
    }
}

/// Polls a detached command until it completes, for at most its own timeout plus a grace period, so a progress run
/// returns what a blocking run would.
pub(super) async fn await_completed(
    client: &SandboxClient,
    lease_id: &LeaseId,
    execution_id: &ExecutionId,
    timeout_seconds: u32,
) -> Result<CommandOutput, ToolError> {
    let patience = std::time::Duration::from_secs(u64::from(timeout_seconds) + COMPLETION_GRACE_SECONDS);
    let give_up = tokio::time::Instant::now() + patience;
    loop {
        match polled(client.poll_command(lease_id, execution_id).await?)? {
            Polled::Completed(output) => return Ok(output),
            Polled::Running => {}
            Polled::NotFound => return Err(ToolError::ExecutionUnknown(execution_id.to_string())),
        }
        if tokio::time::Instant::now() >= give_up {
            return Err(ToolError::ExecutionUnfinished(execution_id.to_string()));
        }
        tokio::time::sleep(COMPLETION_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests;
