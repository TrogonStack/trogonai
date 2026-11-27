mod command_deadline_support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use command_deadline_support::{Reply, Server, TestResult, exec, file_path, lease_sandbox, run_request};
use serde_json::json;
use trogon_sandbox_contract::{LeaseError, ListProcessesResponse, RunCommandResponse};

#[derive(Default)]
struct Process {
    active: AtomicBool,
    stop_attempts: AtomicUsize,
    cleaned: AtomicBool,
    binding: Mutex<Option<(String, String)>>,
    violations: Mutex<Vec<String>>,
}

impl Process {
    fn binding(&self) -> (String, String) {
        self.binding
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_default()
    }

    fn require(&self, held: bool, what: &str) {
        if !held {
            self.violations
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(what.to_owned());
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StopOutcome {
    GuestWatchdogEnd,
    StillRunning,
}

async fn run_stop_race(
    outcome: StopOutcome,
) -> TestResult<(Arc<Process>, RunCommandResponse, ListProcessesResponse, bool)> {
    let process = Arc::new(Process::default());
    let state = process.clone();
    let server = Server::start(move |request| {
        if request.path == "/process.Process/Start" {
            let args = request.args();
            let arg = |index: usize| args.get(index).cloned().unwrap_or_default();
            let script = arg(6);
            if script.contains("nohup setsid") {
                let mut binding = state.binding.lock().unwrap_or_else(PoisonError::into_inner);
                state.require(binding.is_none(), "one bootstrap");
                *binding = Some((arg(8), arg(10)));
                state.active.store(true, Ordering::SeqCst);
                return Reply::process(b"123 456\n", 0);
            }
            if script.starts_with("d=$1; expiry=$2; s=$3; p=$4;") {
                let (directory, expiry) = state.binding();
                state.require(
                    arg(8) == directory
                        && arg(9) == expiry
                        && arg(10) == "KILL"
                        && arg(11) == "123"
                        && arg(12) == "456",
                    "the stop helper is bound to the exact native group",
                );
                if outcome == StopOutcome::GuestWatchdogEnd {
                    let was_live = state.active.swap(false, Ordering::SeqCst);
                    state.require(
                        was_live,
                        "the guest watchdog wins only after the host's live identity probe",
                    );
                }
                state.stop_attempts.fetch_add(1, Ordering::SeqCst);
                return Reply::process(b"", 1);
            }
            if script == "rm -rf -- \"$1\"" {
                state.require(!state.active.load(Ordering::SeqCst), "cleanup follows the native end");
                state.require(arg(8) == state.binding().0, "cleanup targets the staging directory");
                state.cleaned.store(true, Ordering::SeqCst);
                return Reply::process(b"", 0);
            }
            state.require(script.starts_with("p=$1\n") && arg(8) == "123", "a known native script");
            return if state.active.load(Ordering::SeqCst) {
                Reply::process(b"123 456\n", 0)
            } else {
                Reply::process(b"", 44)
            };
        }
        if request.path.starts_with("/files?") {
            let path = file_path(&request);
            let (directory, expiry) = state.binding();
            let ended = !state.active.load(Ordering::SeqCst);
            let name = path.strip_prefix(&format!("{directory}/")).unwrap_or("").to_owned();
            state.require(!name.is_empty(), "receipts live in the staging directory");
            return match name.as_str() {
                "deadline-outcome" if ended => Reply::Complete(200, format!("123 456 {expiry}\n").into_bytes()),
                "exit" if ended => Reply::Complete(200, b"137\n".to_vec()),
                "out" => Reply::Complete(200, b"before deadline".to_vec()),
                "err" => Reply::Complete(200, Vec::new()),
                _ => Reply::json(404, json!({"message": "native receipt not yet present"})),
            };
        }
        state.require(false, "an unexpected envd operation");
        Reply::json(404, json!({}))
    })
    .await?;
    let sandbox = lease_sandbox(&server.url, "deadline-stop-race")?;
    let exec = exec()?;
    let request = run_request(json!({
        "lease_id": "deadline-stop-race", "command": ["sleep", "60"],
        "timeout_seconds": 1, "deadline": "timeout"
    }))?;
    let response = exec.handle_run_command(&sandbox, &request).await;
    let inventory = exec.handle_list_processes(&sandbox, "deadline-stop-race").await;
    let still_running = process.active.load(Ordering::SeqCst);
    exec.forget_lease("deadline-stop-race").await;
    process.active.store(false, Ordering::SeqCst);
    server.shutdown().await;
    let violations = process
        .violations
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(violations.is_empty(), "{violations:?}");
    Ok((process, response, inventory, still_running))
}

#[tokio::test]
async fn foreground_deadline_accepts_exact_guest_watchdog_end_that_wins_the_host_stop_race() -> TestResult {
    let (process, response, inventory, still_running) = run_stop_race(StopOutcome::GuestWatchdogEnd).await?;
    assert!(
        !still_running && process.stop_attempts.load(Ordering::SeqCst) == 1,
        "the exact guest watchdog must win the stop race once"
    );
    assert!(
        matches!(response, RunCommandResponse::Completed(ref output)
        if output.timed_out.is_timed_out() && output.exit_code == -1 && output.stdout == b"before deadline"),
        "confirmed guest deadline end must survive a losing host stop helper: {response:?}"
    );
    assert!(
        process.cleaned.load(Ordering::SeqCst)
            && matches!(inventory, ListProcessesResponse::Processes { ref processes } if processes.is_empty()),
        "confirmed foreground completion must retire staging and inventory: {inventory:?}"
    );
    Ok(())
}

#[tokio::test]
async fn foreground_deadline_rejects_nonzero_host_stop_while_exact_native_group_still_runs() -> TestResult {
    let (process, response, _, still_running) = run_stop_race(StopOutcome::StillRunning).await?;
    assert!(
        still_running && process.stop_attempts.load(Ordering::SeqCst) >= 1 && !process.cleaned.load(Ordering::SeqCst),
        "failed native stop must preserve the live execution and staging"
    );
    assert!(
        matches!(response, RunCommandResponse::Denied(LeaseError::Backend { ref message })
        if message == "finite deadline exact native stop failed: exit=1"),
        "a nonzero stop cannot complete while the original native group still runs: {response:?}"
    );
    Ok(())
}
