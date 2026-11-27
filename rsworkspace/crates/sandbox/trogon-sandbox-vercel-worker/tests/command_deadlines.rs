mod command_support;
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use command_support::{
    CMD, Reply, Server, TestResult, body_of, error_reply, exit_record, is_control, kill_request, locked, log_reply,
    ndjson, run_request, sandbox, started,
};
use serde_json::json;
use trogon_sandbox_contract::{
    CommandDeadline, KillCommandResponse, LeaseError, PollCommandResponse, RunCommandRequest, RunCommandResponse,
    UnixSignal,
};
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelExec};

const LEASE: &str = "deadline-lease";

#[derive(Default)]
struct Process {
    active: AtomicBool,
    starts: AtomicUsize,
    kills: AtomicUsize,
    reject_kills: AtomicBool,
    native_budgets: std::sync::Mutex<Vec<Option<u64>>>,
    capture_id: std::sync::Mutex<Option<String>>,
    control_logs: std::sync::Mutex<BTreeMap<String, String>>,
}

#[derive(Clone, Copy)]
enum Mode {
    NativeTimeout,
    KillRejected,
    IdentityWithheld,
    KillConfirmed,
    KillAcceptedWithoutExit,
    TerminalBeforeStreamEnds(i32),
}

struct Fixture {
    server: Server,
    process: Arc<Process>,
    exec: VercelExec,
    sandbox: LeaseSandbox,
}

/// What the capture control reports for the status it is asked to read back.
fn control_reply(state: &Process, args: &[serde_json::Value]) -> Reply {
    assert_eq!(args[3].as_str(), locked(&state.capture_id).as_deref());
    assert!(!state.active.load(Ordering::SeqCst));
    let (id, data) = match args.get(4).and_then(serde_json::Value::as_str) {
        Some(native) => {
            assert_eq!(native, "137");
            ("capture-read", "137\n0\n\n\n0\n\n\n".to_owned())
        }
        None => ("capture-cleanup", String::new()),
    };
    locked(&state.control_logs).insert(id.to_owned(), data);
    ndjson(&exit_record(id, 0))
}

fn launch_reply(state: &Arc<Process>, mode: Mode, body: &serde_json::Value) -> Reply {
    let args = body["args"].as_array().map(Vec::as_slice).unwrap_or_default();
    assert_eq!(args.len(), 7);
    assert_eq!(args[0], "-c");
    assert_eq!(
        args[2..],
        [
            json!("sandbox-capture"),
            json!("1s"),
            json!("/vercel/sandbox"),
            json!("sleep"),
            json!("60")
        ]
    );
    let script = args[1].as_str().unwrap_or_default();
    let capture = script
        .strip_prefix("set -- ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_default();
    assert_eq!(capture.len(), 32);
    assert!(
        capture
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    );
    *locked(&state.capture_id) = Some(capture.to_owned());
    state.starts.fetch_add(1, Ordering::SeqCst);
    state.active.store(true, Ordering::SeqCst);
    let budget = body["timeout"].as_u64();
    locked(&state.native_budgets).push(budget);
    if let (Mode::NativeTimeout, Some(milliseconds)) = (mode, budget) {
        let state = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(milliseconds)).await;
            state.active.store(false, Ordering::SeqCst);
        });
    }
    if body["wait"] != true {
        return Reply::json(200, exit_record("original-command", serde_json::Value::Null));
    }
    Reply::Held(match mode {
        Mode::IdentityWithheld => Vec::new(),
        Mode::TerminalBeforeStreamEnds(_) => {
            state.active.store(false, Ordering::SeqCst);
            b"{\"command\":{\"id\":\"original-command\",\"exitCode\":0}}\n".to_vec()
        }
        _ => b"{\"command\":{\"id\":\"original-command\",\"exitCode\":null}}\n".to_vec(),
    })
}

impl Fixture {
    async fn new(mode: Mode) -> Result<Self, command_support::Boxed> {
        let process = Arc::new(Process::default());
        process
            .reject_kills
            .store(matches!(mode, Mode::KillRejected), Ordering::SeqCst);
        let state = process.clone();
        let server = Server::start(move |request| {
            let route = request.route();
            if request.method == "POST" && route == CMD {
                let body = body_of(&request);
                let args = body["args"].as_array().map(Vec::as_slice).unwrap_or_default();
                return if is_control(&body) {
                    control_reply(&state, args)
                } else {
                    launch_reply(&state, mode, &body)
                };
            }
            if request.method == "POST" && route.ends_with("/original-command/kill") {
                state.kills.fetch_add(1, Ordering::SeqCst);
                if state.reject_kills.load(Ordering::SeqCst) {
                    return error_reply(503, "termination unavailable");
                }
                if !matches!(mode, Mode::KillAcceptedWithoutExit) {
                    state.active.store(false, Ordering::SeqCst);
                }
                return Reply::json(200, json!({}));
            }
            if request.method == "GET" {
                if let Some(data) = locked(&state.control_logs)
                    .iter()
                    .find_map(|(id, data)| route.ends_with(&format!("/{id}/logs")).then(|| data.clone()))
                {
                    return log_reply(&data);
                }
                if route.ends_with("/original-command/logs") {
                    if let Mode::TerminalBeforeStreamEnds(code) = mode {
                        return log_reply(&format!("{code}\n0\n\n\n0\n\n\n"));
                    }
                    // Native termination can interrupt capture before it emits a frame.
                    return Reply::Complete(200, Vec::new());
                }
                if route.ends_with("/original-command") {
                    let code = if matches!(mode, Mode::TerminalBeforeStreamEnds(_)) {
                        Some(0)
                    } else {
                        (!state.active.load(Ordering::SeqCst)).then_some(137)
                    };
                    return Reply::json(200, exit_record("original-command", code));
                }
            }
            error_reply(404, "unknown fixture operation")
        })
        .await?;
        let exec = server.exec()?;
        Ok(Self {
            server,
            process,
            exec,
            sandbox: sandbox(),
        })
    }

    async fn run(
        &self,
        detached: bool,
        deadline: CommandDeadline,
    ) -> Result<RunCommandResponse, command_support::Boxed> {
        let request = request(detached, deadline)?;
        Ok(self.exec.handle_run_command(&self.sandbox, &request).await)
    }

    async fn poll(&self, lease: &str) -> PollCommandResponse {
        self.exec
            .handle_poll_command(&self.sandbox, lease, "original-command")
            .await
    }

    async fn kill(&self, id: &str) -> KillCommandResponse {
        self.exec
            .handle_kill_command(&self.sandbox, LEASE, &kill_request(LEASE, id, UnixSignal::Kill))
            .await
    }

    fn starts(&self) -> usize {
        self.process.starts.load(Ordering::SeqCst)
    }

    fn kills(&self) -> usize {
        self.process.kills.load(Ordering::SeqCst)
    }

    fn running(&self) -> bool {
        self.process.active.load(Ordering::SeqCst)
    }

    fn budgets(&self) -> Vec<Option<u64>> {
        locked(&self.process.native_budgets).clone()
    }
}

fn request(detached: bool, deadline: CommandDeadline) -> Result<RunCommandRequest, serde_json::Error> {
    run_request(
        LEASE,
        json!(["sleep", "60"]),
        json!({"timeout_seconds": 1, "detached": detached, "deadline": deadline}),
    )
}

#[tokio::test]
async fn detached_timeout_reaches_native_execution_without_polling() -> TestResult {
    let fixture = Fixture::new(Mode::NativeTimeout).await?;
    let id = started(fixture.run(true, CommandDeadline::Timeout).await?)?;
    assert_eq!(fixture.starts(), 1);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    // The documented native timeout is in milliseconds and applies at exec time.
    assert_eq!(fixture.budgets(), vec![Some(1000)]);
    assert!(
        !fixture.running(),
        "a finite detached execution stops without a poll request"
    );
    fixture.kill(id.as_str()).await;
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn detached_lease_deadline_does_not_install_a_finite_native_budget() -> TestResult {
    let fixture = Fixture::new(Mode::NativeTimeout).await?;
    let id = started(fixture.run(true, CommandDeadline::Lease).await?)?;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(fixture.budgets(), vec![None]);
    assert!(fixture.running());
    assert_eq!(fixture.kills(), 0);
    fixture.kill(id.as_str()).await;
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn foreground_timeout_does_not_complete_after_native_kill_failure() -> TestResult {
    let fixture = Fixture::new(Mode::KillRejected).await?;
    let response =
        tokio::time::timeout(Duration::from_secs(10), fixture.run(false, CommandDeadline::Timeout)).await??;
    assert_eq!(fixture.starts(), 1);
    assert_eq!(fixture.kills(), 1, "the exact command id reached the failing kill API");
    assert!(
        fixture.running(),
        "the injected refusal leaves the native fixture running"
    );
    assert!(matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    assert!(matches!(fixture.poll(LEASE).await, PollCommandResponse::Running));
    assert!(matches!(
        fixture.poll("another-lease").await,
        PollCommandResponse::NotFound
    ));
    fixture.process.reject_kills.store(false, Ordering::SeqCst);
    assert_eq!(fixture.kill("original-command").await, KillCommandResponse::Signaled);
    let recovered = fixture.poll(LEASE).await;
    assert!(
        !fixture.process.active.swap(false, Ordering::SeqCst),
        "the retained identity supports explicit cleanup"
    );
    assert!(
        matches!(recovered, PollCommandResponse::Completed(output) if output.exit_code == 137 && !output.timed_out.is_timed_out())
    );
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn accepted_kill_without_a_terminal_receipt_remains_unknown_and_pollable() -> TestResult {
    let fixture = Fixture::new(Mode::KillAcceptedWithoutExit).await?;
    let response = fixture.run(false, CommandDeadline::Timeout).await?;
    let observed = fixture.poll(LEASE).await;
    assert_eq!((fixture.starts(), fixture.kills()), (1, 1));
    assert!(fixture.process.active.swap(false, Ordering::SeqCst));
    assert!(matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    assert!(matches!(observed, PollCommandResponse::Running));
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn accepted_deadline_kill_and_native_terminal_receipt_complete_the_timeout() -> TestResult {
    let fixture = Fixture::new(Mode::KillConfirmed).await?;
    let response = fixture.run(false, CommandDeadline::Timeout).await?;
    assert!(!fixture.process.active.swap(false, Ordering::SeqCst));
    assert_eq!((fixture.starts(), fixture.kills()), (1, 1));
    assert!(
        matches!(response, RunCommandResponse::Completed(output) if output.timed_out.is_timed_out() && output.exit_code == -1)
    );
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn native_terminal_codes_before_a_held_stream_are_not_deadline_causes() -> TestResult {
    for code in [0, 124, 137] {
        let fixture = Fixture::new(Mode::TerminalBeforeStreamEnds(code)).await?;
        let response = fixture.run(false, CommandDeadline::Timeout).await?;
        assert_eq!((fixture.starts(), fixture.kills()), (1, 0));
        assert!(!fixture.process.active.swap(false, Ordering::SeqCst));
        assert!(
            matches!(response, RunCommandResponse::Completed(output) if !output.timed_out.is_timed_out() && output.exit_code == code)
        );
        fixture.server.shutdown().await;
    }
    Ok(())
}

#[tokio::test]
async fn foreground_timeout_without_a_native_id_reports_unknown_execution() -> TestResult {
    let fixture = Fixture::new(Mode::IdentityWithheld).await?;
    let response =
        tokio::time::timeout(Duration::from_secs(10), fixture.run(false, CommandDeadline::Timeout)).await??;
    assert_eq!(
        fixture.starts(),
        1,
        "the launch was accepted before its id was withheld"
    );
    assert!(fixture.process.active.swap(false, Ordering::SeqCst));
    assert_eq!(fixture.kills(), 0, "no id is invented for the unknown execution");
    assert!(matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    fixture.server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn the_response_helpers_reject_other_outcomes() {
    let denied = || RunCommandResponse::Denied(LeaseError::NoProviderAvailable);
    assert!(started(denied()).is_err());
    assert!(command_support::completed(denied()).is_err());
    assert!(command_support::polled(PollCommandResponse::Running).is_err());
}
