mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, ExecutionId, GetSandboxDomainResponse, KillCommandRequest,
    KillCommandResponse, LeaseError, LeaseId, PollCommandResponse, ReadFileRequest, ReadFileResponse,
    RunCommandRequest, RunCommandResponse, UnixSignal, WriteFilesRequest, WriteFilesResponse, WriteStdinResponse,
};
use trogon_sandbox_daytona_worker::{DaytonaExec, LeaseSandbox};

const LEASE: &str = "lease-1";

fn execute_route() -> String {
    format!("/box/{SANDBOX}/process/execute")
}

fn sandbox() -> LeaseSandbox {
    LeaseSandbox { id: SANDBOX.to_owned() }
}

fn run_request(change: impl FnOnce(&mut Value)) -> Result<RunCommandRequest, serde_json::Error> {
    let mut wire = json!({"lease_id": LEASE, "command": ["echo", "hi"], "timeout_seconds": 5});
    change(&mut wire);
    serde_json::from_value(wire)
}

fn kill_request(execution: &str) -> KillCommandRequest {
    KillCommandRequest {
        lease_id: LeaseId::new(LEASE),
        execution_id: ExecutionId::new(execution),
        signal: UnixSignal::Term,
    }
}

/// A toolbox answer: the exit code and the text.
fn done(code: i32, result: &str) -> Reply {
    Reply::json(200, json!({"exitCode": code, "result": result}))
}

fn broken() -> Reply {
    Reply::json(500, json!({"message": "down"}))
}

fn completed(response: RunCommandResponse) -> Result<CommandOutput, Boxed> {
    match response {
        RunCommandResponse::Completed(output) => Ok(output),
        other => Err(format!("not completed: {other:?}").into()),
    }
}

fn is_denied(response: &RunCommandResponse) -> bool {
    matches!(response, RunCommandResponse::Denied(_))
}

async fn started(exec: &DaytonaExec) -> Result<String, Boxed> {
    match exec
        .handle_run_command(&sandbox(), &run_request(|wire| wire["detached"] = json!(true))?)
        .await
    {
        RunCommandResponse::Started(started) => Ok(started.execution_id.as_str().to_owned()),
        other => Err(format!("not started: {other:?}").into()),
    }
}

fn finished_frame() -> String {
    frame("0", b"out", b"err")
}

#[tokio::test]
async fn a_foreground_command_returns_its_separated_output() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script(
        "POST",
        &execute_route(),
        vec![done(0, &frame("7", b"out\n", b"warn\n"))],
    );
    let output = completed(fake.exec()?.handle_run_command(&sandbox(), &run_request(|_| {})?).await)?;
    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, b"out\n");
    assert_eq!(output.stderr, b"warn\n");
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    let calls = fake.calls("POST", &execute_route());
    let body = calls.first().ok_or("no call")?.json();
    assert_eq!(body["timeout"], 15);
    let command = body["command"].as_str().ok_or("no command")?;
    assert!(
        command.contains("sandbox-capture") && command.contains("'echo' 'hi'"),
        "{command}"
    );
    Ok(())
}

#[tokio::test]
async fn a_command_the_deadline_stopped_reports_a_timeout() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("POST", &execute_route(), vec![done(0, &frame("timeout", b"", b""))]);
    let output = completed(fake.exec()?.handle_run_command(&sandbox(), &run_request(|_| {})?).await)?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    Ok(())
}

#[tokio::test]
async fn the_environment_and_user_reach_the_command() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("POST", &execute_route(), vec![done(0, &finished_frame())]);
    let request = run_request(|wire| {
        wire["env"] = json!({"A": "1"});
        wire["cwd"] = json!("sub");
        wire["run_as_user"] = json!("builder");
    })?;
    completed(fake.exec()?.handle_run_command(&sandbox(), &request).await)?;
    let calls = fake.calls("POST", &execute_route());
    let command = calls.first().ok_or("no call")?.json()["command"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        command.contains("'A=1'") && command.contains("'sudo'") && command.contains("/home/builder/sub"),
        "{command}"
    );
    Ok(())
}

#[tokio::test]
async fn a_command_that_cannot_run_is_denied() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let exec = fake.exec()?;
    for change in [
        (|wire: &mut Value| wire["stream_output"] = json!(true)) as fn(&mut Value),
        |wire| wire["command"] = json!([]),
        |wire| wire["env"] = json!({"1BAD": "x"}),
    ] {
        let response = exec.handle_run_command(&sandbox(), &run_request(change)?).await;
        assert!(matches!(
            response,
            RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })
        ));
    }
    assert_eq!(fake.total(), 0);
    fake.script("POST", &execute_route(), vec![done(2, "")]);
    assert!(is_denied(
        &exec.handle_run_command(&sandbox(), &run_request(|_| {})?).await
    ));
    fake.script("POST", &execute_route(), vec![done(0, "garbage")]);
    assert!(is_denied(
        &exec.handle_run_command(&sandbox(), &run_request(|_| {})?).await
    ));
    fake.script("POST", &execute_route(), vec![broken()]);
    assert!(is_denied(
        &exec.handle_run_command(&sandbox(), &run_request(|_| {})?).await
    ));
    Ok(())
}

#[tokio::test]
async fn a_detached_command_starts_polls_and_is_cleaned_up() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script(
        "POST",
        &execute_route(),
        vec![done(0, ""), done(3, ""), done(0, &finished_frame()), done(0, "")],
    );
    let exec = fake.exec()?;
    let id = started(&exec).await?;
    let calls = fake.calls("POST", &execute_route());
    let command = calls.first().ok_or("no start call")?.json()["command"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(command.contains("nohup") && command.contains(&id), "{command}");
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, &id).await,
        PollCommandResponse::Running
    ));
    match exec.handle_poll_command(&sandbox(), LEASE, &id).await {
        PollCommandResponse::Completed(output) => assert_eq!(output.stdout, b"out"),
        other => return Err(format!("not completed: {other:?}").into()),
    }
    assert_eq!(fake.count("POST", &execute_route()), 4);
    // The result is kept: asking again costs no call.
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, &id).await,
        PollCommandResponse::Completed(_)
    ));
    assert_eq!(fake.count("POST", &execute_route()), 4);
    assert!(matches!(
        exec.handle_kill_command(&sandbox(), LEASE, &kill_request(&id)).await,
        KillCommandResponse::AlreadyCompleted
    ));
    Ok(())
}

#[tokio::test]
async fn a_detached_command_that_cannot_start_is_denied() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let exec = fake.exec()?;
    let request = run_request(|wire| wire["detached"] = json!(true))?;
    fake.script("POST", &execute_route(), vec![done(1, "")]);
    assert!(is_denied(&exec.handle_run_command(&sandbox(), &request).await));
    fake.script("POST", &execute_route(), vec![broken()]);
    assert!(is_denied(&exec.handle_run_command(&sandbox(), &request).await));
    Ok(())
}

#[tokio::test]
async fn polling_what_this_lease_did_not_start_finds_nothing() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("POST", &execute_route(), vec![done(0, "")]);
    let exec = fake.exec()?;
    let id = started(&exec).await?;
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), "other", &id).await,
        PollCommandResponse::NotFound
    ));
    assert!(matches!(
        exec.handle_poll_command(
            &LeaseSandbox {
                id: "elsewhere".to_owned()
            },
            LEASE,
            &id
        )
        .await,
        PollCommandResponse::NotFound
    ));
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, "nope").await,
        PollCommandResponse::NotFound
    ));
    assert!(matches!(
        exec.handle_kill_command(&sandbox(), LEASE, &kill_request("nope")).await,
        KillCommandResponse::NotFound
    ));
    exec.forget_lease(LEASE).await;
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, &id).await,
        PollCommandResponse::NotFound
    ));
    Ok(())
}

#[tokio::test]
async fn a_poll_that_fails_is_denied() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let exec = fake.exec()?;
    for (reply, label) in [(broken(), "api"), (done(5, ""), "exit"), (done(0, "garbage"), "frame")] {
        fake.script("POST", &execute_route(), vec![done(0, "")]);
        let id = started(&exec).await?;
        fake.script("POST", &execute_route(), vec![reply]);
        let response = exec.handle_poll_command(&sandbox(), LEASE, &id).await;
        assert!(matches!(response, PollCommandResponse::Denied(_)), "{label}");
    }
    Ok(())
}

#[tokio::test]
async fn a_cleanup_that_fails_does_not_lose_the_result() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("POST", &execute_route(), vec![done(0, "")]);
    let exec = fake.exec()?;
    let id = started(&exec).await?;
    fake.script("POST", &execute_route(), vec![done(0, &finished_frame()), broken()]);
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, &id).await,
        PollCommandResponse::Completed(_)
    ));
    Ok(())
}

#[tokio::test]
async fn killing_signals_the_marked_processes() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("POST", &execute_route(), vec![done(0, "")]);
    let exec = fake.exec()?;
    let id = started(&exec).await?;
    fake.script("POST", &execute_route(), vec![done(0, "")]);
    assert!(matches!(
        exec.handle_kill_command(&sandbox(), LEASE, &kill_request(&id)).await,
        KillCommandResponse::Signaled
    ));
    let calls = fake.calls("POST", &execute_route());
    let command = calls.last().ok_or("no kill call")?.json()["command"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(command.contains(&format!("SANDBOX_EXECUTION_ID={id}")), "{command}");
    Ok(())
}

#[tokio::test]
async fn killing_what_nothing_matches_checks_whether_it_already_finished() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script("POST", &execute_route(), vec![done(0, "")]);
    let exec = fake.exec()?;
    let id = started(&exec).await?;
    let cases = [
        (vec![done(1, ""), done(0, &finished_frame())], "completed"),
        (vec![done(1, ""), done(3, "")], "no process"),
        (vec![done(1, ""), broken()], "poll failed"),
        (vec![done(2, "")], "odd exit"),
        (vec![broken()], "api"),
    ];
    for (replies, label) in cases {
        fake.script("POST", &execute_route(), replies);
        let response = exec.handle_kill_command(&sandbox(), LEASE, &kill_request(&id)).await;
        let expected = match label {
            "completed" => matches!(response, KillCommandResponse::AlreadyCompleted),
            _ => matches!(response, KillCommandResponse::Denied(_)),
        };
        assert!(expected, "{label}: {response:?}");
    }
    Ok(())
}

#[tokio::test]
async fn expired_results_are_dropped_when_the_next_command_starts() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.script(
        "POST",
        &execute_route(),
        vec![done(0, ""), done(0, &finished_frame()), done(0, ""), done(0, "")],
    );
    let exec = fake.exec()?.with_result_ttl(Duration::ZERO);
    let first = started(&exec).await?;
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, &first).await,
        PollCommandResponse::Completed(_)
    ));
    tokio::time::sleep(Duration::from_millis(5)).await;
    started(&exec).await?;
    assert!(matches!(
        exec.handle_poll_command(&sandbox(), LEASE, &first).await,
        PollCommandResponse::NotFound
    ));
    Ok(())
}

fn write_request(files: Value, as_user: Value) -> Result<WriteFilesRequest, serde_json::Error> {
    serde_json::from_value(json!({"lease_id": LEASE, "files": files, "as_user": as_user}))
}

#[tokio::test]
async fn written_files_upload_into_the_workspace_with_their_mode() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.json("POST", &format!("/box/{SANDBOX}/files/bulk-upload"), 200, json!({}));
    fake.json("POST", &format!("/box/{SANDBOX}/files/permissions"), 200, json!({}));
    let files = json!([
        {"path": "a.txt", "content": [104, 105], "mode": 0o100_750},
        {"path": "/abs/b.txt", "content": [1]},
    ]);
    let response = fake
        .exec()?
        .handle_write_files(&sandbox(), &write_request(files, Value::Null)?)
        .await;
    assert!(matches!(response, WriteFilesResponse::Written));
    let uploads = fake.calls("POST", &format!("/box/{SANDBOX}/files/bulk-upload"));
    assert_eq!(uploads.len(), 2);
    assert!(uploads[0].text().contains("/workspace/a.txt"));
    assert!(uploads[1].text().contains("/abs/b.txt"));
    let modes = fake.calls("POST", &format!("/box/{SANDBOX}/files/permissions"));
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0].query, "path=%2Fworkspace%2Fa.txt&mode=750");
    Ok(())
}

#[tokio::test]
async fn a_write_that_cannot_be_served_is_denied() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let exec = fake.exec()?;
    let one = json!([{"path": "a", "content": [1], "mode": 420}]);
    let denied = |response| matches!(response, WriteFilesResponse::Denied(_));
    assert!(denied(
        exec.handle_write_files(&sandbox(), &write_request(one.clone(), json!("root"))?)
            .await
    ));
    assert!(denied(
        exec.handle_write_files(
            &sandbox(),
            &write_request(json!([{"path": "/", "content": []}]), Value::Null)?
        )
        .await
    ));
    fake.json("POST", &format!("/box/{SANDBOX}/files/bulk-upload"), 500, json!({}));
    assert!(denied(
        exec.handle_write_files(&sandbox(), &write_request(one.clone(), Value::Null)?)
            .await
    ));
    fake.json("POST", &format!("/box/{SANDBOX}/files/bulk-upload"), 200, json!({}));
    fake.json("POST", &format!("/box/{SANDBOX}/files/permissions"), 500, json!({}));
    assert!(denied(
        exec.handle_write_files(&sandbox(), &write_request(one, Value::Null)?)
            .await
    ));
    Ok(())
}

#[tokio::test]
async fn reads_return_content_or_report_absence() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let route = format!("/box/{SANDBOX}/files/download");
    fake.raw("GET", &route, 200, b"hello");
    let exec = fake.exec()?;
    let request = |path: &str, as_user: Value| -> Result<ReadFileRequest, serde_json::Error> {
        serde_json::from_value(json!({"lease_id": LEASE, "path": path, "as_user": as_user}))
    };
    assert!(matches!(
        exec.handle_read_file(&sandbox(), &request("a.txt", Value::Null)?).await,
        ReadFileResponse::Content { ref data } if data == b"hello"
    ));
    assert_eq!(fake.calls("GET", &route)[0].query, "path=%2Fworkspace%2Fa.txt");
    assert!(matches!(
        exec.handle_read_file(&sandbox(), &request("a", json!("root"))?).await,
        ReadFileResponse::Denied(_)
    ));
    fake.json("GET", &route, 404, json!({}));
    assert!(matches!(
        exec.handle_read_file(&sandbox(), &request("a", Value::Null)?).await,
        ReadFileResponse::NotFound
    ));
    fake.json("GET", &route, 500, json!({}));
    assert!(matches!(
        exec.handle_read_file(&sandbox(), &request("a", Value::Null)?).await,
        ReadFileResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_domain_is_the_preview_url_with_its_token() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let route = format!("/sandbox/{SANDBOX}/port-preview/3000");
    fake.json(
        "GET",
        &route,
        200,
        json!({"url": "https://3000-abc.proxy.example/", "token": "tok"}),
    );
    let exec = fake.exec()?;
    match exec.handle_get_domain(&sandbox(), 3000).await {
        GetSandboxDomainResponse::Available { endpoint, access, .. } => {
            assert_eq!(endpoint, "3000-abc.proxy.example:443");
            let access = access.ok_or("no access")?;
            assert_eq!(access.base_url, "https://3000-abc.proxy.example");
            assert_eq!(
                access.headers.get("x-daytona-preview-token").map(String::as_str),
                Some("tok")
            );
        }
        other => return Err(format!("not available: {other:?}").into()),
    }
    fake.json("GET", &route, 200, json!({"url": "http://open.example:8080"}));
    assert!(matches!(
        exec.handle_get_domain(&sandbox(), 3000).await,
        GetSandboxDomainResponse::Available { ref endpoint, ref access, .. }
            if endpoint == "open.example:8080" && access.as_ref().is_some_and(|a| a.headers.is_empty())
    ));
    Ok(())
}

#[tokio::test]
async fn a_domain_that_cannot_be_served_is_denied() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let exec = fake.exec()?;
    assert!(matches!(
        exec.handle_get_domain(&sandbox(), 0).await,
        GetSandboxDomainResponse::Denied(_)
    ));
    let route = format!("/sandbox/{SANDBOX}/port-preview/80");
    fake.json("GET", &route, 500, json!({}));
    assert!(matches!(
        exec.handle_get_domain(&sandbox(), 80).await,
        GetSandboxDomainResponse::Denied(_)
    ));
    for url in ["not a url", "mailto:x@example.com"] {
        fake.json("GET", &route, 200, json!({"url": url}));
        assert!(
            matches!(
                exec.handle_get_domain(&sandbox(), 80).await,
                GetSandboxDomainResponse::Denied(_)
            ),
            "{url}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn stdin_is_not_supported() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    assert!(matches!(
        fake.exec()?.handle_write_stdin(),
        WriteStdinResponse::Unsupported
    ));
    Ok(())
}
