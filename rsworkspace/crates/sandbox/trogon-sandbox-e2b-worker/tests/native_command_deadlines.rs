#![cfg(target_os = "linux")]

mod native_command_support;
mod native_storage_support;

use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::time::Duration;

use native_command_support::{Binding, Guest, NativeEnvd};
use native_storage_support::{Boxed, TestResult, exec, lease_sandbox, run_request, started};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::Instant;
use trogon_sandbox_contract::CommandDeadline;
use trogon_std::env::{ReadEnv, SystemEnv};

const STARTED: &str = "E2B_NATIVE_EXECUTION_STARTED ";
const CHILD_URL: &str = "TROGON_NATIVE_WORKER_URL";
const CHILD_DEADLINE: &str = "TROGON_NATIVE_WORKER_DEADLINE";
const CHILD_TEST: &str = "native_worker_child";

/// Starts a detached command against the native fixture at `endpoint`, announces its execution and then idles for
/// `hold` as the worker would until it is lost.
async fn worker(endpoint: &str, deadline: CommandDeadline, hold: Duration, out: &mut impl Write) -> TestResult {
    let sandbox = lease_sandbox(endpoint, "owned-native-fixture")?;
    let request = run_request(json!({
        "lease_id": "native-worker-loss", "command": ["bash", "-c", "sleep 60 & wait"],
        "detached": true, "timeout_seconds": 2, "deadline": deadline, "run_as_user": "root"
    }))?;
    let response = exec()?.handle_run_command(&sandbox, &request).await;
    writeln!(out, "{STARTED}{}", started(&response)?)?;
    out.flush()?;
    tokio::time::sleep(hold).await;
    Ok(())
}

/// The worker the parent test loses. Run on its own it exercises the same path against a private fixture and
/// returns at once; the parent test points it at its fixture and kills it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_worker_child() -> TestResult {
    let remote = SystemEnv.var(CHILD_URL).ok();
    let deadline: CommandDeadline = SystemEnv
        .var(CHILD_DEADLINE)
        .ok()
        .map(|text| serde_json::from_str(&text))
        .transpose()?
        .unwrap_or_default();
    let guest = Guest::create()?;
    let local = NativeEnvd::start(guest.clone()).await?;
    let endpoint = remote.clone().unwrap_or_else(|| local.url.clone());
    let hold = Duration::from_secs(remote.map_or(0, |_| 3600));
    let result = worker(&endpoint, deadline, hold, &mut std::io::stdout()).await;
    local.shutdown().await;
    guest.cleanup()?;
    result
}

async fn prove_worker_loss(deadline: CommandDeadline) -> TestResult {
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(guest.clone()).await?;
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_URL, &envd.url)
        .env(CHILD_DEADLINE, serde_json::to_string(&deadline)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("the worker has no stdout")?).lines();
    let announced = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let line = lines
                .next_line()
                .await?
                .ok_or("the worker exited before it started a command")?;
            if let Some((_, execution)) = line.split_once(STARTED) {
                break Ok::<_, Boxed>(execution.trim().to_owned());
            }
        }
    })
    .await?;
    let observed = observe_loss(&guest, &mut child, deadline).await;
    envd.shutdown().await;
    guest.cleanup()?;
    let (binding, before, remaining) = observed?;
    assert!(!announced?.is_empty(), "the worker announced the execution it started");
    assert!(
        match deadline {
            CommandDeadline::Timeout => remaining.is_empty(),
            CommandDeadline::Lease => remaining.contains(&binding.pid) && remaining.len() >= 2,
        },
        "native command lifetime after whole-worker loss is wrong: deadline={deadline:?}, exact_binding={binding:?}, before={before:?}, remaining={remaining:?}"
    );
    Ok(())
}

async fn observe_loss(
    guest: &Guest,
    child: &mut tokio::process::Child,
    deadline: CommandDeadline,
) -> TestResult<(Binding, Vec<u32>, Vec<u32>)> {
    let binding = *guest
        .bindings
        .lock()
        .map_err(|_| "the binding record is poisoned")?
        .first()
        .ok_or("the bootstrap did not record a native process identity")?;
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let members = guest.group_members(binding)?;
            if members.len() >= 2 && members.contains(&binding.pid) {
                break Ok::<_, Boxed>(members);
            }
        }
    })
    .await??;
    child.kill().await?;
    let killed = child.wait().await?;
    assert_eq!(killed.signal(), Some(9), "the worker did not die by SIGKILL: {killed}");
    let mut remaining = ready.clone();
    let until = Instant::now() + Duration::from_secs(4);
    while Instant::now() < until {
        remaining = guest.group_members(binding)?;
        if deadline == CommandDeadline::Timeout && remaining.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok((binding, ready, remaining))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finite_detached_deadline_survives_whole_worker_loss() -> TestResult {
    prove_worker_loss(CommandDeadline::Timeout).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_lease_deadline_survives_whole_worker_loss_without_expiring() -> TestResult {
    prove_worker_loss(CommandDeadline::Lease).await
}
