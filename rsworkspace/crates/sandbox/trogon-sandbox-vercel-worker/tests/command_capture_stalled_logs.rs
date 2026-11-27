mod command_support;
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use command_support::{
    CMD, Reply, Server, TestResult, body_of, exec_within, exit_record, is_control, is_read, locked, log_reply, polled,
    run_request, sandbox, started, terminal,
};
use serde_json::json;
use trogon_sandbox_contract::{CommandOutput, PollCommandResponse};

const LEASE: &str = "stalled-log-lease";
const FRAME: &str = "9\n3\nAP/+\n\n2\ngP8=\n\n";

#[derive(Default)]
struct Native {
    user_starts: AtomicUsize,
    reads: AtomicUsize,
    cleanups: AtomicUsize,
    user_budgets: std::sync::Mutex<Vec<Option<u64>>>,
}

fn assert_output(output: &CommandOutput) {
    assert_eq!(output.exit_code, 9);
    assert_eq!(output.stdout, [0, 255, 254]);
    assert_eq!(output.stderr, [128, 255]);
    command_support::assert_clean(output);
}

async fn stalled_log_case(detached: bool) -> TestResult {
    let native = Arc::new(Native::default());
    let state = native.clone();
    let server = Server::start(move |request| {
        let route = request.route();
        if request.method == "POST" && route == CMD {
            let body = body_of(&request);
            let id = if !is_control(&body) {
                state.user_starts.fetch_add(1, Ordering::SeqCst);
                locked(&state.user_budgets).push(body["timeout"].as_u64());
                "original-command"
            } else if is_read(&body) {
                state.reads.fetch_add(1, Ordering::SeqCst);
                "capture-read"
            } else {
                state.cleanups.fetch_add(1, Ordering::SeqCst);
                "capture-cleanup"
            };
            return terminal(&body, &exit_record(id, 0));
        }
        if request.method == "GET" && route.ends_with("/original-command") {
            return Reply::json(200, exit_record("original-command", 0));
        }
        if request.method == "GET" && route.ends_with("/original-command/logs") {
            return Reply::Held(Vec::new());
        }
        if request.method == "GET" && route.ends_with("/capture-read/logs") {
            return log_reply(FRAME);
        }
        Reply::Complete(200, Vec::new())
    })
    .await?;
    // The transport timeout must not finish the stalled stream before the worker's own bound does.
    let exec = exec_within(&server, 60)?;
    let sandbox = sandbox();
    let command = json!(["sh", "-c", "printf '\\000\\377\\376'; printf '\\200\\377' >&2; exit 9"]);
    let request = run_request(LEASE, command, json!({"timeout_seconds": 1, "detached": detached}))?;
    let started_at = tokio::time::Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(35), exec.handle_run_command(&sandbox, &request)).await?;
    let (output, repeated) = if detached {
        let id = started(response)?;
        let poll = tokio::time::timeout(
            Duration::from_secs(35),
            exec.handle_poll_command(&sandbox, LEASE, id.as_str()),
        );
        let output = polled(poll.await?)?;
        let repeated = exec.handle_poll_command(&sandbox, LEASE, id.as_str()).await;
        (output, Some(repeated))
    } else {
        (command_support::completed(response)?, None)
    };
    assert!(
        started_at.elapsed() < Duration::from_secs(30),
        "the fallback shares the 30s output deadline"
    );
    assert_eq!(native.user_starts.load(Ordering::SeqCst), 1);
    assert_eq!(*locked(&native.user_budgets), [Some(1000)]);
    assert_eq!(native.reads.load(Ordering::SeqCst), 1);
    assert_eq!(native.cleanups.load(Ordering::SeqCst), 1);
    assert_output(&output);
    if let Some(repeated) = repeated {
        assert_eq!(repeated, PollCommandResponse::Completed(output));
    }
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn foreground_healthy_retained_read_is_not_starved_by_an_unending_original_log() -> TestResult {
    stalled_log_case(false).await
}

#[tokio::test]
async fn detached_healthy_retained_read_is_not_starved_by_an_unending_original_log() -> TestResult {
    stalled_log_case(true).await
}
