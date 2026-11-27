use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, LeaseError, LeaseId, PollCommandResponse, RunCommandResponse, RunCommandStartedResponse,
};
use trogon_sandbox_protocol::subjects;

use super::support::{call, call_with};
use crate::policy::{CommandPolicy, Policy};
use crate::testing::{McpSession, Reply, ScriptedBus, TestResult, test_server};

fn finished(code: i32) -> CommandOutput {
    CommandOutput {
        exit_code: code,
        stdout: b"hi".to_vec(),
        stderr: Vec::new(),
        ..CommandOutput::default()
    }
}

fn started() -> RunCommandResponse {
    RunCommandResponse::Started(RunCommandStartedResponse {
        execution_id: ExecutionId::new("e1"),
        origin: Default::default(),
    })
}

fn run_args(extra: Value) -> Value {
    let mut args = json!({"lease_id": "l1", "command": ["echo", "hi"], "timeout_seconds": 10});
    if let (Some(base), Value::Object(more)) = (args.as_object_mut(), extra) {
        base.extend(more);
    }
    args
}

#[tokio::test]
async fn an_attached_command_returns_its_output() -> TestResult {
    let outcome = call(
        "sandbox_run_command",
        run_args(json!({})),
        &RunCommandResponse::Completed(finished(0)),
    )
    .await?;
    assert!(!outcome.is_error());
    let text = outcome.text()?;
    assert!(text.contains("\"completed\""), "{text}");
    assert!(text.contains("aGk="), "{text}");
    Ok(())
}

#[tokio::test]
async fn a_detached_command_returns_the_execution_id() -> TestResult {
    let outcome = call("sandbox_run_command", run_args(json!({"detached": true})), &started()).await?;
    assert!(!outcome.is_error());
    assert!(outcome.text()?.contains("e1"));
    Ok(())
}

#[tokio::test]
async fn a_launch_the_broker_refuses_is_an_error() -> TestResult {
    let denied = RunCommandResponse::Denied(LeaseError::NoProviderAvailable);
    let outcome = call("sandbox_run_command", run_args(json!({})), &denied).await?;
    assert!(outcome.is_error());
    Ok(())
}

#[tokio::test]
async fn a_wrong_kind_of_launch_reply_is_an_error() -> TestResult {
    let attached_got_started = call("sandbox_run_command", run_args(json!({})), &started()).await?;
    assert!(attached_got_started.is_error());
    let detached_got_output = call(
        "sandbox_run_command",
        run_args(json!({"detached": true})),
        &RunCommandResponse::Completed(finished(0)),
    )
    .await?;
    assert!(detached_got_output.is_error());
    Ok(())
}

#[tokio::test]
async fn an_impossible_launch_never_reaches_the_bus() -> TestResult {
    let outcome = call(
        "sandbox_run_command",
        run_args(json!({"stdin": "interactive"})),
        &started(),
    )
    .await?;
    assert!(outcome.is_error());
    assert!(outcome.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_command_policy_vetoes_before_anything_is_sent() -> TestResult {
    let policy = Policy {
        commands: CommandPolicy::new(&["^echo".to_string()], &[])?,
        ..Policy::default()
    };
    let replies = vec![Reply::json(&started())?];
    let outcome = call_with(policy, "sandbox_run_command", run_args(json!({})), replies).await?;
    assert!(outcome.is_error());
    assert!(outcome.sent.is_empty());
    Ok(())
}

fn logs(chunks: Value, next_seq: u64) -> Value {
    json!({"outcome": "logs", "chunks": chunks, "next_seq": next_seq, "exited": true})
}

#[tokio::test]
async fn a_progress_run_reports_output_and_returns_the_result() -> TestResult {
    let chunk = json!([{"log_seq": 1, "stream": "stdout", "seq": 1, "data": [104, 105]}]);
    let replies = vec![
        Reply::json(&started())?,
        Reply::json(&logs(chunk, 1))?,
        Reply::json(&logs(json!([]), 1))?,
        Reply::json(&PollCommandResponse::Completed(finished(0)))?,
    ];
    let bus = ScriptedBus::scripted(replies);
    let session = McpSession::start(test_server(bus, Policy::default())?).await?;
    let args = run_args(json!({"stream_progress": true}));
    let result = session.call_tracked("sandbox_run_command", args, Some("tok")).await?;
    assert_ne!(result.is_error, Some(true));
    let progress = session.recorder().progress(1).await;
    assert_eq!(progress.len(), 1);
    assert!(progress.iter().any(|m| m.contains("hi")), "{progress:?}");
    session.finish().await?;
    Ok(())
}

#[tokio::test]
async fn a_progress_run_whose_command_never_finishes_is_an_error() -> TestResult {
    let replies = vec![
        Reply::json(&started())?,
        Reply::json(&logs(json!([]), 0))?,
        Reply::json(&logs(json!([]), 0))?,
        Reply::json(&PollCommandResponse::NotFound)?,
    ];
    let session = McpSession::start(test_server(ScriptedBus::scripted(replies), Policy::default())?).await?;
    let args = run_args(json!({"stream_progress": true}));
    let result = session.call_tracked("sandbox_run_command", args, Some("tok")).await?;
    assert_eq!(result.is_error, Some(true));
    session.finish().await?;
    Ok(())
}

#[tokio::test]
async fn polling_reports_every_state() -> TestResult {
    let args = json!({"lease_id": "l1", "execution_id": "e1"});
    let running = call("sandbox_poll_command", args.clone(), &PollCommandResponse::Running).await?;
    assert!(running.text()?.contains("running"));
    let missing = call("sandbox_poll_command", args.clone(), &PollCommandResponse::NotFound).await?;
    assert!(missing.text()?.contains("not_found"));
    let done = call(
        "sandbox_poll_command",
        args.clone(),
        &PollCommandResponse::Completed(finished(3)),
    )
    .await?;
    assert!(done.text()?.contains("\"exit_code\":3"));
    let sent = done.sent.first().ok_or("nothing was sent")?;
    assert_eq!(sent.subject, subjects::lease_poll_command(&LeaseId::new("l1")));
    Ok(())
}

#[tokio::test]
async fn process_tools_pass_their_replies_through() -> TestResult {
    let base = json!({"lease_id": "l1"});
    let with = |extra: Value| -> Value {
        let mut args = base.clone();
        if let (Some(target), Value::Object(more)) = (args.as_object_mut(), extra) {
            target.extend(more);
        }
        args
    };
    let listed = call(
        "sandbox_list_processes",
        base.clone(),
        &json!({"outcome": "unsupported"}),
    )
    .await?;
    assert!(listed.text()?.contains("unsupported"));

    let ready = call(
        "sandbox_wait_ready",
        with(json!({"execution_id": "e1", "timeout_seconds": 1})),
        &json!({"outcome": "ready"}),
    )
    .await?;
    assert!(ready.text()?.contains("ready"));
    let default_wait = call(
        "sandbox_wait_ready",
        with(json!({"execution_id": "e1"})),
        &json!({"outcome": "not_ready"}),
    )
    .await?;
    assert!(default_wait.text()?.contains("not_ready"));

    let by_name = call(
        "sandbox_kill_process",
        with(json!({"name": "web", "signal": "kill"})),
        &json!({"outcome": "not_found"}),
    )
    .await?;
    assert!(by_name.text()?.contains("not_found"));
    let bad_name = call(
        "sandbox_kill_process",
        with(json!({"name": "no spaces"})),
        &json!({"outcome": "not_found"}),
    )
    .await?;
    assert!(bad_name.is_error());
    let bad_signal = call(
        "sandbox_kill_process",
        with(json!({"name": "web", "signal": "boom"})),
        &json!({"outcome": "not_found"}),
    )
    .await?;
    assert!(bad_signal.is_error());
    assert!(bad_signal.sent.is_empty());

    let killed = call(
        "sandbox_kill_command",
        with(json!({"execution_id": "e1"})),
        &json!({"outcome": "signaled"}),
    )
    .await?;
    assert!(killed.text()?.contains("signaled"));
    let stopped = call(
        "sandbox_stop_command",
        with(json!({"execution_id": "e1", "grace_seconds": 2})),
        &json!({"outcome": "unsupported"}),
    )
    .await?;
    assert!(stopped.text()?.contains("unsupported"));
    let restarted = call(
        "sandbox_restart_command",
        with(json!({"execution_id": "e1"})),
        &json!({"outcome": "restarted", "execution_id": "e2"}),
    )
    .await?;
    assert!(restarted.text()?.contains("e2"));
    Ok(())
}

#[tokio::test]
async fn reading_logs_pages_and_names_the_streams() -> TestResult {
    let chunks = json!([
        {"log_seq": 1, "stream": "stdout", "seq": 1, "data": [111]},
        {"log_seq": 2, "stream": "stderr", "seq": 1, "data": [101]}
    ]);
    let reply = json!({"outcome": "logs", "chunks": chunks, "next_seq": 2, "has_more": true, "truncated": true});
    let args = json!({"lease_id": "l1", "execution_id": "e1", "after_seq": 0, "max_bytes": 100});
    let page = call("sandbox_read_logs", args.clone(), &reply).await?;
    let text = page.text()?;
    assert!(text.contains("stdout") && text.contains("stderr"), "{text}");
    assert!(
        text.contains("\"has_more\":true") && text.contains("\"truncated\":true"),
        "{text}"
    );

    let none = call("sandbox_read_logs", args, &json!({"outcome": "unsupported"})).await?;
    assert!(none.text()?.contains("unsupported"));
    Ok(())
}

#[tokio::test]
async fn writing_stdin_checks_its_input() -> TestResult {
    let args = |data: &str, eof: bool| json!({"lease_id": "l1", "execution_id": "e1", "seq": 0, "data_base64": data, "eof": eof});
    let written = call(
        "sandbox_write_stdin",
        args("aGk=", false),
        &json!({"outcome": "written", "next_seq": 1}),
    )
    .await?;
    assert!(written.text()?.contains("written"));
    let closing = call(
        "sandbox_write_stdin",
        args("", true),
        &json!({"outcome": "stdin_closed"}),
    )
    .await?;
    assert!(!closing.is_error());
    let empty = call(
        "sandbox_write_stdin",
        args("", false),
        &json!({"outcome": "stdin_closed"}),
    )
    .await?;
    assert!(empty.is_error());
    let garbage = call(
        "sandbox_write_stdin",
        args("***", false),
        &json!({"outcome": "stdin_closed"}),
    )
    .await?;
    assert!(garbage.is_error());
    assert!(empty.sent.is_empty() && garbage.sent.is_empty());
    Ok(())
}
