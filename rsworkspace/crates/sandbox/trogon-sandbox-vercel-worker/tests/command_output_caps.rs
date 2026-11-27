mod command_support;
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use command_support::{
    CMD, Reply, Server, TestResult, body_of, error_reply, exit_record, is_control, is_read, log_reply, ndjson, polled,
    run_request, sandbox, started, terminal,
};
use serde_json::{Value, json};
use trogon_sandbox_contract::{CommandDeadline, CommandOutput, LeaseError, PollCommandResponse, RunCommandResponse};
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelExec};

const LEASE: &str = "native-output-cap";

/// Both streams of `stdout()` and `stderr()` as the guest capture frame encodes them.
const FRAME: &str = "7\n64\nSEVBRGFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhVEFJTA==\n\n64\nRVJSSGJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiRVJSVA==\n\n";

#[derive(Default)]
struct Native {
    starts: AtomicUsize,
    log_reads: AtomicUsize,
    reject_logs: AtomicBool,
}

struct Fixture {
    server: Server,
    native: Arc<Native>,
    sandbox: LeaseSandbox,
    exec: VercelExec,
}

fn stdout() -> String {
    format!("HEAD{}TAIL", "a".repeat(56))
}

fn stderr() -> String {
    format!("ERRH{}ERRT", "b".repeat(56))
}

fn assert_capped(output: &CommandOutput) {
    assert_eq!(output.exit_code, 7);
    assert!(!output.timed_out.is_timed_out());
    assert!(output.truncated.is_truncated());
    assert_eq!(output.stdout, b"HEAD\n[... 56 bytes truncated ...]\nTAIL");
    assert_eq!(output.stderr, b"ERRH\n[... 56 bytes truncated ...]\nERRT");
}

fn assert_whole(output: &CommandOutput) {
    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, stdout().into_bytes());
    assert_eq!(output.stderr, stderr().into_bytes());
    command_support::assert_clean(output);
}

impl Fixture {
    async fn new() -> Result<Self, command_support::Boxed> {
        let native = Arc::new(Native::default());
        let state = native.clone();
        let server = Server::start(move |request| {
            let route = request.route();
            if request.method == "POST" && route == CMD {
                let body = body_of(&request);
                if is_control(&body) {
                    let read = is_read(&body);
                    if read && state.reject_logs.load(Ordering::SeqCst) {
                        return error_reply(503, "native output reads unavailable");
                    }
                    return ndjson(&exit_record(if read { "capture-read" } else { "capture-cleanup" }, 0));
                }
                state.starts.fetch_add(1, Ordering::SeqCst);
                return terminal(&body, &exit_record("original-command", 0));
            }
            if request.method == "GET" && route.ends_with("/original-command/logs") {
                state.log_reads.fetch_add(1, Ordering::SeqCst);
                return if state.reject_logs.load(Ordering::SeqCst) {
                    error_reply(503, "native logs unavailable")
                } else {
                    log_reply(FRAME)
                };
            }
            if request.method == "GET" && route.ends_with("/capture-read/logs") {
                return log_reply(FRAME);
            }
            if request.method == "GET" && route.ends_with("/capture-cleanup/logs") {
                return Reply::Complete(200, Vec::new());
            }
            if request.method == "GET" && route.ends_with("/original-command") {
                return Reply::json(200, exit_record("original-command", 0));
            }
            error_reply(404, "unknown native fixture operation")
        })
        .await?;
        let exec = server.exec()?;
        Ok(Self {
            server,
            native,
            sandbox: sandbox(),
            exec,
        })
    }

    async fn run(
        &self,
        detached: bool,
        deadline: CommandDeadline,
        cap: Option<u64>,
    ) -> Result<RunCommandResponse, command_support::Boxed> {
        let mut extra = json!({"detached": detached, "deadline": deadline});
        if let Some(cap) = cap {
            extra["max_output_bytes"] = Value::from(cap);
        }
        let command = json!([
            "sh",
            "-c",
            "printf '%s' \"$1\"; printf '%s' \"$2\" >&2; exit 7",
            "sh",
            stdout(),
            stderr()
        ]);
        let request = run_request(LEASE, command, extra)?;
        Ok(self.exec.handle_run_command(&self.sandbox, &request).await)
    }

    async fn poll(&self, lease: &str, id: &str) -> PollCommandResponse {
        self.exec.handle_poll_command(&self.sandbox, lease, id).await
    }
}

#[tokio::test]
async fn detached_requested_cap_survives_native_log_failure_and_retry() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.native.reject_logs.store(true, Ordering::SeqCst);
    let id = started(fixture.run(true, CommandDeadline::Timeout, Some(8)).await?)?;
    let failed = fixture.poll(LEASE, id.as_str()).await;
    fixture.native.reject_logs.store(false, Ordering::SeqCst);
    let recovered = fixture.poll(LEASE, id.as_str()).await;
    let foreign = fixture.poll("another-lease", id.as_str()).await;
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.native.log_reads.load(Ordering::SeqCst), 2);
    assert!(matches!(
        failed,
        PollCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    assert!(matches!(foreign, PollCommandResponse::NotFound));
    assert_capped(&polled(recovered)?);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn explicit_lease_deadline_preserves_the_requested_output_cap() -> TestResult {
    let fixture = Fixture::new().await?;
    let id = started(fixture.run(true, CommandDeadline::Lease, Some(8)).await?)?;
    let observed = fixture.poll(LEASE, id.as_str()).await;
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 1);
    assert_capped(&polled(observed)?);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn default_detached_cap_keeps_small_native_output() -> TestResult {
    let fixture = Fixture::new().await?;
    let id = started(fixture.run(true, CommandDeadline::Timeout, None).await?)?;
    let observed = fixture.poll(LEASE, id.as_str()).await;
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 1);
    assert_whole(&polled(observed)?);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn foreground_requested_cap_preserves_native_head_and_tail() -> TestResult {
    let fixture = Fixture::new().await?;
    let response = fixture.run(false, CommandDeadline::Timeout, Some(8)).await?;
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 1);
    assert_capped(&command_support::completed(response)?);
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn foreground_requested_cap_keeps_small_native_output() -> TestResult {
    let fixture = Fixture::new().await?;
    let response = fixture.run(false, CommandDeadline::Timeout, Some(1024)).await?;
    assert_eq!(fixture.native.starts.load(Ordering::SeqCst), 1);
    assert_whole(&command_support::completed(response)?);
    fixture.server.shutdown().await;
    Ok(())
}
