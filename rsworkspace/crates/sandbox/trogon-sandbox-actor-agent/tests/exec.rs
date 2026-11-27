#![cfg(feature = "server")]

use std::collections::BTreeMap;
use std::error::Error;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use trogon_sandbox_actor_agent::{ExecFrame, ExecRegistry, LeaseBinding};
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, KillCommandResponse, LeaseError, LeaseId, PollCommandResponse, RunCommandRequest,
    RunCommandResponse, StdinChunk, StdinSeq, UnixSignal, WriteStdinRequest, WriteStdinResponse,
};

type TestResult = Result<(), Box<dyn Error>>;

fn binding() -> LeaseBinding {
    LeaseBinding {
        lease_id: LeaseId::new("01J0"),
        env: BTreeMap::new(),
        working_dir: None,
        exposed_port: None,
    }
}

fn request(command: &[&str], extra: Value) -> Result<RunCommandRequest, Box<dyn Error>> {
    let mut body = json!({"lease_id": "01J0", "command": command});
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    Ok(serde_json::from_value(body)?)
}

async fn collect(mut receiver: mpsc::Receiver<ExecFrame>) -> Vec<ExecFrame> {
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        frames.push(frame);
    }
    frames
}

async fn run(registry: &ExecRegistry, command: &[&str], extra: Value) -> Result<Vec<ExecFrame>, Box<dyn Error>> {
    Ok(collect(registry.run(request(command, extra)?, binding()).await).await)
}

fn completed(frames: &[ExecFrame]) -> Result<&CommandOutput, Box<dyn Error>> {
    match frames.last() {
        Some(ExecFrame::Response(RunCommandResponse::Completed(output))) => Ok(output),
        other => Err(format!("expected a completed response, got {other:?}").into()),
    }
}

fn started(frames: &[ExecFrame]) -> Result<ExecutionId, Box<dyn Error>> {
    match frames {
        [ExecFrame::Response(RunCommandResponse::Started(started))] => Ok(started.execution_id.clone()),
        other => Err(format!("expected a started response, got {other:?}").into()),
    }
}

fn is_invalid_spec(frames: &[ExecFrame]) -> bool {
    matches!(
        frames,
        [ExecFrame::Response(RunCommandResponse::Denied(
            LeaseError::InvalidSpec { .. }
        ))]
    )
}

async fn completed_output(registry: &ExecRegistry, id: &ExecutionId) -> Result<CommandOutput, Box<dyn Error>> {
    for _ in 0..4000 {
        match registry.poll(id).await {
            PollCommandResponse::Running => tokio::time::sleep(Duration::from_millis(10)).await,
            PollCommandResponse::Completed(output) => return Ok(output),
            other => return Err(format!("unexpected poll response {other:?}").into()),
        }
    }
    Err("the command never completed".into())
}

fn stdin(id: &ExecutionId, seq: u64, data: &[u8], eof: bool) -> Result<WriteStdinRequest, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "lease_id": "01J0",
        "execution_id": id.as_str(),
        "seq": seq,
        "data": data,
        "eof": eof,
    }))?)
}

fn next(seq: u64) -> StdinSeq {
    StdinSeq::new(seq)
}

#[tokio::test]
async fn synchronous_command_returns_stdout_and_exit_code() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "printf hello"], json!({})).await?;
    assert_eq!(frames.len(), 1, "got {frames:?}");
    let output = completed(&frames)?;
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"hello");
    Ok(())
}

#[tokio::test]
async fn non_zero_exit_code_is_reported_verbatim() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "exit 7"], json!({})).await?;
    assert_eq!(completed(&frames)?.exit_code, 7);
    Ok(())
}

#[tokio::test]
async fn stderr_is_captured_separately() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "printf oops >&2"], json!({})).await?;
    let output = completed(&frames)?;
    assert_eq!(output.stderr, b"oops");
    assert!(output.stdout.is_empty());
    Ok(())
}

#[tokio::test]
async fn bound_env_is_visible_and_per_call_env_wins() -> TestResult {
    let registry = ExecRegistry::new();
    let mut bound = binding();
    bound.env.insert("AGENT_SCOPE".to_string(), "lease".to_string());
    bound.env.insert("AGENT_ONLY".to_string(), "lease".to_string());
    let command = ["/bin/sh", "-c", "printf %s-%s $AGENT_SCOPE $AGENT_ONLY"];
    let request = request(&command, json!({"env": {"AGENT_SCOPE": "call"}}))?;
    let frames = collect(registry.run(request, bound).await).await;
    assert_eq!(completed(&frames)?.stdout, b"call-lease");
    Ok(())
}

#[tokio::test]
async fn a_relative_cwd_resolves_against_the_bound_working_dir() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    let registry = ExecRegistry::new();
    let mut bound = binding();
    bound.working_dir = Some(dir.path().to_string_lossy().to_string());
    let request = request(&["/bin/sh", "-c", "pwd"], json!({"cwd": "sub"}))?;
    let frames = collect(registry.run(request, bound).await).await;
    let output = completed(&frames)?;
    assert_eq!(output.exit_code, 0, "got {output:?}");
    assert!(String::from_utf8(output.stdout.clone())?.trim().ends_with("/sub"));
    Ok(())
}

#[tokio::test]
async fn a_missing_cwd_does_not_run_the_command() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(
        &registry,
        &["/bin/sh", "-c", "echo ran"],
        json!({"cwd": "/nonexistent/cwd"}),
    )
    .await?;
    let output = completed(&frames)?;
    assert_eq!(output.exit_code, 1);
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"cd: /nonexistent/cwd: No such file or directory");
    Ok(())
}

#[tokio::test]
async fn cwd_falls_back_to_the_bound_working_dir() -> TestResult {
    let dir = tempfile::tempdir()?;
    let registry = ExecRegistry::new();
    let mut bound = binding();
    bound.working_dir = Some(dir.path().to_string_lossy().to_string());
    let request = request(&["/bin/sh", "-c", "pwd"], json!({}))?;
    let frames = collect(registry.run(request, bound).await).await;
    let stdout = String::from_utf8(completed(&frames)?.stdout.clone())?;
    let name = dir
        .path()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    assert!(stdout.trim().ends_with(&name), "got {stdout}");
    Ok(())
}

#[tokio::test]
async fn streaming_emits_chunks_before_the_response() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(
        &registry,
        &["/bin/sh", "-c", "printf hello"],
        json!({"stream_output": true}),
    )
    .await?;
    assert!(frames.len() >= 3, "got {frames:?}");
    assert!(matches!(frames[0], ExecFrame::Begin(_)));
    assert!(matches!(frames[1], ExecFrame::Chunk(_)));
    assert!(matches!(
        frames.last(),
        Some(ExecFrame::Response(RunCommandResponse::Completed(_)))
    ));
    Ok(())
}

#[tokio::test]
async fn streaming_a_silent_command_still_names_its_execution() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "true"], json!({"stream_output": true})).await?;
    assert!(
        matches!(
            frames.as_slice(),
            [
                ExecFrame::Begin(_),
                ExecFrame::Response(RunCommandResponse::Completed(_))
            ]
        ),
        "got {frames:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_command_that_does_not_stream_sends_no_begin_frame() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "true"], json!({})).await?;
    assert!(
        !frames.iter().any(|frame| matches!(frame, ExecFrame::Begin(_))),
        "got {frames:?}"
    );
    Ok(())
}

async fn start_interactive(registry: &ExecRegistry, command: &[&str]) -> Result<ExecutionId, Box<dyn Error>> {
    started(&run(registry, command, json!({"detached": true, "stdin": "interactive"})).await?)
}

#[tokio::test]
async fn interactive_stdin_reaches_the_process_until_eof() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/cat"]).await?;
    let first = registry.write_stdin(&stdin(&id, 0, b"hello ", false)?).await;
    assert_eq!(first, WriteStdinResponse::Written { next_seq: next(1) });
    let last = registry.write_stdin(&stdin(&id, 1, b"world", true)?).await;
    assert_eq!(last, WriteStdinResponse::Written { next_seq: next(2) });
    let output = completed_output(&registry, &id).await?;
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"hello world");
    Ok(())
}

#[tokio::test]
async fn repeated_sequence_is_a_duplicate_and_not_written_twice() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/cat"]).await?;
    registry.write_stdin(&stdin(&id, 0, b"once", false)?).await;
    let again = registry.write_stdin(&stdin(&id, 0, b"once", false)?).await;
    assert_eq!(again, WriteStdinResponse::Duplicate { next_seq: next(1) });
    registry.write_stdin(&stdin(&id, 1, b"", true)?).await;
    assert_eq!(completed_output(&registry, &id).await?.stdout, b"once");
    Ok(())
}

#[tokio::test]
async fn write_after_eof_is_closed() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/sleep", "30"]).await?;
    registry.write_stdin(&stdin(&id, 0, b"", true)?).await;
    let late = registry.write_stdin(&stdin(&id, 1, b"late", false)?).await;
    assert_eq!(late, WriteStdinResponse::StdinClosed);
    registry.kill(&id, UnixSignal::Kill).await;
    Ok(())
}

#[tokio::test]
async fn ahead_of_order_write_is_buffered_then_applied_in_order() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/cat"]).await?;
    let early = registry.write_stdin(&stdin(&id, 1, b"b", true)?).await;
    assert_eq!(early, WriteStdinResponse::Buffered { next_seq: next(0) });
    let ordered = registry.write_stdin(&stdin(&id, 0, b"a", false)?).await;
    assert_eq!(ordered, WriteStdinResponse::Written { next_seq: next(2) });
    assert_eq!(completed_output(&registry, &id).await?.stdout, b"ab");
    Ok(())
}

#[tokio::test]
async fn a_write_far_ahead_of_the_window_is_out_of_window() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/sleep", "30"]).await?;
    let response = registry.write_stdin(&stdin(&id, 1_000_000, b"x", false)?).await;
    assert!(
        matches!(response, WriteStdinResponse::OutOfWindow { .. }),
        "got {response:?}"
    );
    registry.kill(&id, UnixSignal::Kill).await;
    Ok(())
}

#[tokio::test]
async fn a_process_that_stops_reading_applies_backpressure_or_closes() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/sleep", "30"]).await?;
    let chunk = vec![b'x'; StdinChunk::MAX_LEN];
    let mut verdict = None;
    for seq in 0..4096 {
        let response = registry.write_stdin(&stdin(&id, seq, &chunk, false)?).await;
        if !matches!(response, WriteStdinResponse::Written { .. }) {
            verdict = Some(response);
            break;
        }
    }
    assert!(
        matches!(
            verdict,
            Some(WriteStdinResponse::Backpressure { .. } | WriteStdinResponse::StdinClosed)
        ),
        "got {verdict:?}"
    );
    registry.kill(&id, UnixSignal::Kill).await;
    Ok(())
}

#[tokio::test]
async fn write_to_a_command_without_interactive_stdin_is_not_interactive() -> TestResult {
    let registry = ExecRegistry::new();
    let id = started(&run(&registry, &["/bin/sleep", "30"], json!({"detached": true})).await?)?;
    let response = registry.write_stdin(&stdin(&id, 0, b"x", false)?).await;
    assert_eq!(response, WriteStdinResponse::StdinNotInteractive);
    registry.kill(&id, UnixSignal::Kill).await;
    Ok(())
}

#[tokio::test]
async fn write_to_an_unknown_execution_is_not_found() -> TestResult {
    let registry = ExecRegistry::new();
    let response = registry
        .write_stdin(&stdin(&ExecutionId::new("nope"), 0, b"x", false)?)
        .await;
    assert_eq!(response, WriteStdinResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn write_to_a_finished_execution_is_already_completed() -> TestResult {
    let registry = ExecRegistry::new();
    let id = start_interactive(&registry, &["/bin/sh", "-c", "exit 0"]).await?;
    completed_output(&registry, &id).await?;
    let response = registry.write_stdin(&stdin(&id, 0, b"x", false)?).await;
    assert_eq!(response, WriteStdinResponse::AlreadyCompleted);
    Ok(())
}

#[tokio::test]
async fn stream_only_retention_keeps_no_stdout() -> TestResult {
    let registry = ExecRegistry::new();
    let command = ["/bin/sh", "-c", "printf out; printf err >&2"];
    let id = started(
        &run(
            &registry,
            &command,
            json!({"detached": true, "output_retention": "stream_only"}),
        )
        .await?,
    )?;
    let output = completed_output(&registry, &id).await?;
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"err");
    Ok(())
}

#[tokio::test]
async fn an_attached_command_cannot_ask_for_a_lease_deadline() {
    let refused = request(&["/bin/sleep", "60"], json!({"deadline": "lease"}));
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_detached_lease_deadline_is_denied() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(
        &registry,
        &["/bin/sleep", "60"],
        json!({"detached": true, "deadline": "lease"}),
    )
    .await?;
    assert!(is_invalid_spec(&frames), "got {frames:?}");
    Ok(())
}

#[tokio::test]
async fn detached_command_answers_immediately_and_is_pollable() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(
        &registry,
        &["/bin/sh", "-c", "printf detached"],
        json!({"detached": true}),
    )
    .await?;
    assert_eq!(frames.len(), 1, "got {frames:?}");
    let id = started(&frames)?;
    assert_eq!(completed_output(&registry, &id).await?.stdout, b"detached");
    Ok(())
}

#[tokio::test]
async fn polling_an_unknown_execution_is_not_found() {
    let registry = ExecRegistry::new();
    assert_eq!(
        registry.poll(&ExecutionId::new("nope")).await,
        PollCommandResponse::NotFound
    );
}

#[tokio::test]
async fn killing_an_unknown_execution_is_not_found() {
    let registry = ExecRegistry::new();
    assert_eq!(
        registry.kill(&ExecutionId::new("nope"), UnixSignal::Term).await,
        KillCommandResponse::NotFound
    );
}

#[tokio::test]
async fn kill_terminates_a_detached_command() -> TestResult {
    let registry = ExecRegistry::new();
    let id = started(
        &run(
            &registry,
            &["/bin/sh", "-c", "sleep 60"],
            json!({"detached": true, "timeout_seconds": 120}),
        )
        .await?,
    )?;
    assert_eq!(
        registry.kill(&id, UnixSignal::Kill).await,
        KillCommandResponse::Signaled
    );
    completed_output(&registry, &id).await?;
    assert_eq!(
        registry.kill(&id, UnixSignal::Kill).await,
        KillCommandResponse::AlreadyCompleted
    );
    Ok(())
}

#[tokio::test]
async fn timeout_kills_the_command_and_reports_it() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "sleep 30"], json!({"timeout_seconds": 1})).await?;
    let output = completed(&frames)?;
    assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("timed out"),
        "got {output:?}"
    );
    assert!(output.timed_out.is_timed_out());
    Ok(())
}

#[tokio::test]
async fn output_over_the_cap_keeps_head_and_tail_and_flags_truncation() -> TestResult {
    let registry = ExecRegistry::new();
    let script = "printf HEAD; i=0; while [ $i -lt 2000 ]; do printf xxxxxxxxxx; i=$((i+1)); done; printf TAIL";
    let frames = run(&registry, &["/bin/sh", "-c", script], json!({"max_output_bytes": 1024})).await?;
    let output = completed(&frames)?;
    assert_eq!(output.exit_code, 0);
    assert!(output.truncated.is_truncated(), "got {} bytes", output.stdout.len());
    assert!(output.stdout.starts_with(b"HEAD"));
    assert!(output.stdout.ends_with(b"TAIL"));
    assert!(output.stdout.len() < 1024 + 64, "got {}", output.stdout.len());
    assert!(output.timed_out.is_within_deadline());
    Ok(())
}

#[tokio::test]
async fn output_within_the_cap_is_not_flagged() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/bin/sh", "-c", "printf hello"], json!({})).await?;
    let output = completed(&frames)?;
    assert_eq!(output.stdout, b"hello");
    assert!(output.truncated.is_complete());
    Ok(())
}

#[tokio::test]
async fn empty_command_is_denied() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &[], json!({})).await?;
    assert!(is_invalid_spec(&frames), "got {frames:?}");
    Ok(())
}

#[tokio::test]
async fn run_as_a_user_that_does_not_exist_is_an_invalid_spec() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(
        &registry,
        &["/bin/sh", "-c", "true"],
        json!({"run_as_user": "definitely-not-a-user-xyz"}),
    )
    .await?;
    assert!(is_invalid_spec(&frames), "got {frames:?}");
    Ok(())
}

#[tokio::test]
async fn run_as_user_runs_the_command_under_that_user() -> TestResult {
    let uid = nix::unistd::Uid::effective();
    let Some(current) = nix::unistd::User::from_uid(uid)? else {
        return Ok(());
    };
    let registry = ExecRegistry::new();
    let script = "id -u; printf %s \"$HOME\"";
    let frames = run(
        &registry,
        &["/bin/sh", "-c", script],
        json!({"run_as_user": current.name}),
    )
    .await?;
    let output = completed(&frames)?;
    let stdout = String::from_utf8(output.stdout.clone())?;
    assert_eq!(output.exit_code, 0, "got {output:?}");
    assert!(stdout.starts_with(&format!("{}\n", uid.as_raw())), "got {stdout}");
    assert!(stdout.ends_with(current.dir.to_string_lossy().as_ref()), "got {stdout}");
    Ok(())
}

#[tokio::test]
async fn a_missing_binary_completes_with_a_failure_rather_than_hanging() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(&registry, &["/nonexistent/binary"], json!({})).await?;
    let output = completed(&frames)?;
    assert_eq!(output.exit_code, 127);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to start"),
        "got {output:?}"
    );
    Ok(())
}

#[tokio::test]
async fn streamed_stderr_is_numbered_separately_from_stdout() -> TestResult {
    let registry = ExecRegistry::new();
    let frames = run(
        &registry,
        &["/bin/sh", "-c", "printf out; printf err >&2"],
        json!({"stream_output": true}),
    )
    .await?;
    let chunks = frames
        .iter()
        .filter(|frame| matches!(frame, ExecFrame::Chunk(_)))
        .count();
    assert_eq!(chunks, 2, "got {frames:?}");
    Ok(())
}
