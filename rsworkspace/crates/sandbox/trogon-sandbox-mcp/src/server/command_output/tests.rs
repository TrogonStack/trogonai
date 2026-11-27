use std::time::Duration;

use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, LeaseError, LeaseId, LogChunk, LogsContinuation, OutputStream, OutputTruncation,
    PollCommandResponse, ReadLogsResponse, RunCommandResponse, RunCommandStartedResponse,
};

use super::{
    Polled, attached_output, await_completed, completed_output, logs_output, poll_output, polled, started_execution_id,
};
use crate::inputs::command_inputs::{PollCommandOutput, ReadLogsOutput, RunCommandOutput};
use crate::testing::{Reply, ScriptedBus, TestResult};
use crate::tool_error::ToolError;

fn output() -> CommandOutput {
    CommandOutput {
        exit_code: 3,
        stdout: b"out".to_vec(),
        stderr: b"err".to_vec(),
        ..CommandOutput::default()
    }
}

fn refusal() -> LeaseError {
    LeaseError::NoProviderAvailable
}

fn started() -> RunCommandResponse {
    RunCommandResponse::Started(RunCommandStartedResponse {
        execution_id: ExecutionId::new("e-1"),
        origin: Default::default(),
    })
}

#[test]
fn a_completed_command_is_base64_encoded() -> TestResult {
    let RunCommandOutput::Completed {
        exit_code,
        stdout_base64,
        stderr_base64,
    } = completed_output(&output())
    else {
        return Err("expected a completed output".into());
    };
    assert_eq!(
        (exit_code, stdout_base64.as_str(), stderr_base64.as_str()),
        (3, "b3V0", "ZXJy")
    );
    Ok(())
}

#[test]
fn a_foreground_launch_returns_its_output_or_an_error() {
    assert!(matches!(attached_output(RunCommandResponse::Completed(output())), Ok(o) if o.exit_code == 3));
    assert!(matches!(attached_output(started()), Err(ToolError::UnexpectedReply(_))));
    assert!(matches!(
        attached_output(RunCommandResponse::Denied(refusal())),
        Err(ToolError::Client(_))
    ));
}

#[test]
fn a_detached_launch_returns_its_execution_id_or_an_error() {
    assert!(matches!(started_execution_id(started()), Ok(id) if id.as_str() == "e-1"));
    assert!(matches!(
        started_execution_id(RunCommandResponse::Completed(output())),
        Err(ToolError::UnexpectedReply(_))
    ));
    assert!(matches!(
        started_execution_id(RunCommandResponse::Denied(refusal())),
        Err(ToolError::Client(_))
    ));
}

#[test]
fn a_poll_maps_each_status() {
    assert!(matches!(
        poll_output(PollCommandResponse::Running),
        Ok(PollCommandOutput::Running)
    ));
    assert!(matches!(
        poll_output(PollCommandResponse::NotFound),
        Ok(PollCommandOutput::NotFound)
    ));
    assert!(matches!(
        poll_output(PollCommandResponse::Completed(output())),
        Ok(PollCommandOutput::Completed { exit_code: 3, .. })
    ));
    assert!(matches!(
        poll_output(PollCommandResponse::Denied(refusal())),
        Err(ToolError::Client(_))
    ));
    assert!(matches!(polled(PollCommandResponse::Running), Ok(Polled::Running)));
}

#[test]
fn logs_keep_their_stream_and_cursor() -> TestResult {
    let chunk = |seq, stream| LogChunk {
        log_seq: seq,
        stream,
        seq,
        data: b"x".to_vec(),
    };
    let response = ReadLogsResponse::Logs {
        chunks: vec![chunk(1, OutputStream::Stdout), chunk(2, OutputStream::Stderr)],
        next_seq: 3,
        has_more: LogsContinuation::HasMore,
        truncated: OutputTruncation::Truncated,
        exited: Default::default(),
    };
    let Ok(ReadLogsOutput::Logs {
        chunks,
        next_seq,
        has_more,
        truncated,
    }) = logs_output(response)
    else {
        return Err("expected logs".into());
    };
    let streams: Vec<&str> = chunks.iter().map(|c| c.stream.as_str()).collect();
    assert_eq!(
        (streams, next_seq, has_more, truncated),
        (vec!["stdout", "stderr"], 3, true, true)
    );
    assert!(matches!(
        logs_output(ReadLogsResponse::Unsupported),
        Ok(ReadLogsOutput::Unsupported)
    ));
    assert!(matches!(
        logs_output(ReadLogsResponse::Denied(refusal())),
        Err(ToolError::Client(_))
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_returns_the_output_once_the_command_completes() -> TestResult {
    let bus = ScriptedBus::scripted(vec![
        Reply::json(&PollCommandResponse::Running)?,
        Reply::json(&PollCommandResponse::Completed(output()))?,
    ]);
    let log = bus.log();
    let client = bus.into_client();
    let done = await_completed(&client, &LeaseId::new("l-1"), &ExecutionId::new("e-1"), 5).await?;
    assert_eq!(done.exit_code, 3);
    assert_eq!(log.all().len(), 2);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_reports_a_command_the_worker_forgot() -> TestResult {
    let client = ScriptedBus::replying(&PollCommandResponse::NotFound)?.into_client();
    let result = await_completed(&client, &LeaseId::new("l-1"), &ExecutionId::new("e-1"), 5).await;
    assert!(matches!(result, Err(ToolError::ExecutionUnknown(id)) if id == "e-1"));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_gives_up_after_the_timeout_and_the_grace() -> TestResult {
    let client = ScriptedBus::replying(&PollCommandResponse::Running)?.into_client();
    let started_at = tokio::time::Instant::now();
    let result = await_completed(&client, &LeaseId::new("l-1"), &ExecutionId::new("e-1"), 1).await;
    assert!(matches!(result, Err(ToolError::ExecutionUnfinished(id)) if id == "e-1"));
    assert!(started_at.elapsed() >= Duration::from_secs(31));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_surfaces_a_broker_refusal() -> TestResult {
    let client = ScriptedBus::replying(&PollCommandResponse::Denied(refusal()))?.into_client();
    let result = await_completed(&client, &LeaseId::new("l-1"), &ExecutionId::new("e-1"), 1).await;
    assert!(matches!(result, Err(ToolError::Client(_))));
    Ok(())
}
