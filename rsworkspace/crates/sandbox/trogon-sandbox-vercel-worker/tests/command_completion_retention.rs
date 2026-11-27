mod command_support;
mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use command_support::{
    CMD, Reply, Server, TestResult, body_of, error_reply, exit_record, is_control, is_read, log_reply, polled,
    run_request, sandbox, started, terminal,
};
use serde_json::json;
use trogon_sandbox_contract::CommandOutput;
use trogon_sandbox_vercel_worker::{LeaseSandbox, VercelExec};

struct ObservedAllocator;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

const LEASE: &str = "retention-lease";
const PAYLOAD_BYTES: usize = 1024 * 1024;
const COMPLETIONS: usize = 16;

async fn settle_connection_tasks() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

async fn complete_and_drop(exec: &VercelExec, sandbox: &LeaseSandbox) -> TestResult {
    let command = json!(["sh", "-c", "head -c 1048576 /dev/zero | tr '\\000' a; exit 9"]);
    let request = run_request(
        LEASE,
        command,
        json!({"detached": true, "timeout_seconds": 1, "max_output_bytes": PAYLOAD_BYTES}),
    )?;
    let id = started(exec.handle_run_command(sandbox, &request).await)?;
    let output: CommandOutput = polled(exec.handle_poll_command(sandbox, LEASE, id.as_str()).await)?;
    assert_eq!(output.exit_code, 9);
    assert_eq!(output.stdout.len(), PAYLOAD_BYTES);
    assert!(output.stdout.iter().all(|byte| *byte == b'a'));
    assert!(output.stderr.is_empty());
    command_support::assert_clean(&output);
    Ok(())
}

/// Whether `id` names a command this fixture issued: `user-N`, `read-N` or `cleanup-N`.
fn issued(id: &str, users: usize, controls: usize) -> bool {
    let Some((role, number)) = id.split_once('-') else {
        return false;
    };
    let limit = if role == "user" { users } else { controls };
    matches!(role, "user" | "read" | "cleanup") && number.parse::<usize>().is_ok_and(|n| n > 0 && n <= limit)
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_public_detached_completions_does_not_retain_their_payloads_per_command() -> TestResult {
    let frame = Arc::new(format!(
        "9\n{PAYLOAD_BYTES}\n{}YQ==\n\n0\n\n\n",
        "YWFh".repeat(PAYLOAD_BYTES / 3)
    ));
    let user_starts = Arc::new(AtomicUsize::new(0));
    let control_starts = Arc::new(AtomicUsize::new(0));
    let (users, controls) = (user_starts.clone(), control_starts.clone());
    let server = Server::start(move |request| {
        let route = request.route();
        if request.method == "POST" && route == CMD {
            let body = body_of(&request);
            let id = if is_control(&body) {
                let number = controls.fetch_add(1, Ordering::SeqCst) + 1;
                format!("{}-{number}", if is_read(&body) { "read" } else { "cleanup" })
            } else {
                format!("user-{}", users.fetch_add(1, Ordering::SeqCst) + 1)
            };
            return terminal(&body, &exit_record(&id, 0));
        }
        let suffix = route
            .strip_prefix("/v2/sandboxes/sessions/session/cmd/")
            .unwrap_or_default();
        let (id, logs) = suffix.strip_suffix("/logs").map_or((suffix, false), |id| (id, true));
        if request.method != "GET" || !issued(id, users.load(Ordering::SeqCst), controls.load(Ordering::SeqCst)) {
            return error_reply(404, "unknown retention fixture operation");
        }
        match (logs, id.starts_with("cleanup-")) {
            (true, true) => Reply::Complete(200, Vec::new()),
            (true, false) => log_reply(&frame),
            (false, _) => Reply::json(200, exit_record(id, 0)),
        }
    })
    .await?;
    let exec = server.exec()?;
    let sandbox = sandbox();
    complete_and_drop(&exec, &sandbox).await?;
    settle_connection_tasks().await;
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    for _ in 0..COMPLETIONS {
        complete_and_drop(&exec, &sandbox).await?;
    }
    settle_connection_tasks().await;
    let retained = LIVE_BYTES.load(Ordering::Relaxed).saturating_sub(baseline);
    assert_eq!(user_starts.load(Ordering::SeqCst), COMPLETIONS + 1);
    assert!(
        retained < 8 * PAYLOAD_BYTES,
        "{COMPLETIONS} completions retained {retained} live bytes after the caller dropped their outputs"
    );
    server.shutdown().await;
    Ok(())
}
