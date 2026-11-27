mod command_deadline_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use command_deadline_support::{
    Reply, Server, TestResult, exec, file_path, kill_request, lease_sandbox, run_request, started,
};
use serde_json::{Value, json};
use trogon_sandbox_contract::{CommandDeadline, PollCommandResponse, RunCommandRequest, RunCommandResponse};
use trogon_sandbox_e2b_worker::{E2bExec, LeaseSandbox};

#[derive(Default)]
struct Process {
    active: AtomicBool,
    starts: AtomicUsize,
    kills: AtomicUsize,
    stopped: tokio::sync::Notify,
    kill_attempted: tokio::sync::Notify,
    fail_kills: AtomicBool,
    keep_running_after_kill: AtomicBool,
}

async fn fixture() -> TestResult<(Server, Arc<Process>, E2bExec, LeaseSandbox)> {
    let process = Arc::new(Process::default());
    let state = process.clone();
    let server = Server::start(move |request| {
        let args = request.args();
        let script = args.get(6).map_or("", String::as_str);
        let has_arg = |wanted: &str| args.iter().any(|arg| arg == wanted);
        if request.path == "/process.Process/Start" {
            if script.contains("nohup setsid") {
                state.starts.fetch_add(1, Ordering::SeqCst);
                state.active.store(true, Ordering::SeqCst);
                return Reply::process(b"123 456\n", 0);
            }
            if script.contains("kill") {
                // The kill is bound to the original start ticks.
                state.kills.fetch_add(
                    has_arg("123").then_some(has_arg("456")).map_or(0, usize::from),
                    Ordering::SeqCst,
                );
                state.kill_attempted.notify_one();
                if state.fail_kills.load(Ordering::SeqCst) {
                    return Reply::json(503, json!({"message": "fixture exact native kill unavailable"}));
                }
                if !state.keep_running_after_kill.load(Ordering::SeqCst) {
                    state.active.store(false, Ordering::SeqCst);
                    state.stopped.notify_one();
                }
                return Reply::process(b"", 0);
            }
            return if state.active.load(Ordering::SeqCst) {
                Reply::process(b"123 456\n", 0)
            } else {
                Reply::process(b"", 44)
            };
        }
        if request.path == "/process.Process/SendSignal" {
            let body: Value = request.json();
            let exact = body["process"]["pid"] == 123 && body["signal"] == "SIGNAL_SIGKILL";
            state.kills.fetch_add(usize::from(exact), Ordering::SeqCst);
            state.active.store(false, Ordering::SeqCst);
            state.stopped.notify_one();
            return Reply::json(200, json!({}));
        }
        if request.path.starts_with("/files?") {
            return if file_path(&request).ends_with("/exit") {
                if state.active.load(Ordering::SeqCst) {
                    Reply::json(404, json!({"code": 404, "message": "not found"}))
                } else {
                    Reply::Complete(200, b"137\n".to_vec())
                }
            } else {
                Reply::Complete(200, Vec::new())
            };
        }
        Reply::json(404, json!({"message": "unknown native fixture operation"}))
    })
    .await?;
    let sandbox = lease_sandbox(&server.url, "native-fixture")?;
    Ok((server, process, exec()?, sandbox))
}

fn request(deadline: CommandDeadline) -> TestResult<RunCommandRequest> {
    run_request(json!({
        "lease_id": "deadline-lease", "command": ["sleep", "60"], "timeout_seconds": 1,
        "detached": true, "deadline": deadline
    }))
}

#[tokio::test]
async fn detached_timeout_stops_the_exact_native_process_without_polling() -> TestResult {
    let (server, process, exec, sandbox) = fixture().await?;
    let response = exec
        .handle_run_command(&sandbox, &request(CommandDeadline::Timeout)?)
        .await;
    let execution = started(&response)?;
    assert_eq!(process.starts.load(Ordering::SeqCst), 1);
    let stopped_without_poll = tokio::time::timeout(Duration::from_secs(4), process.stopped.notified())
        .await
        .is_ok();
    let observed = exec
        .handle_poll_command(&sandbox, "deadline-lease", execution.as_str())
        .await;
    let kills_before_cleanup = process.kills.load(Ordering::SeqCst);
    exec.handle_kill_command(&sandbox, "deadline-lease", &kill_request("deadline-lease", execution))
        .await;
    server.shutdown().await;
    assert!(
        stopped_without_poll,
        "finite detached execution must be supervised without polling"
    );
    assert_eq!(kills_before_cleanup, 1);
    assert!(
        matches!(observed, PollCommandResponse::Completed(output) if output.timed_out.is_timed_out() && output.exit_code == -1)
    );
    Ok(())
}

#[tokio::test]
async fn detached_lease_deadline_keeps_running_past_the_command_timeout() -> TestResult {
    let (server, process, exec, sandbox) = fixture().await?;
    let response = exec
        .handle_run_command(&sandbox, &request(CommandDeadline::Lease)?)
        .await;
    assert!(matches!(response, RunCommandResponse::Started(_)), "{response:?}");
    let execution = started(&response)?;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let still_running = process.active.load(Ordering::SeqCst);
    let kills_before_cleanup = process.kills.load(Ordering::SeqCst);
    exec.handle_kill_command(&sandbox, "deadline-lease", &kill_request("deadline-lease", execution))
        .await;
    server.shutdown().await;
    assert!(still_running);
    assert_eq!(kills_before_cleanup, 0);
    Ok(())
}

#[path = "command_deadlines/failures.rs"]
mod failures_tests;
