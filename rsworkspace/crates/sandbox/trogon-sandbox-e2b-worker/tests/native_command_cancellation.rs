#![cfg(target_os = "linux")]

mod native_command_support;
mod native_storage_support;

use std::time::Duration;

use native_command_support::{Binding, Guest, NativeEnvd};
use native_storage_support::{Boxed, TestResult, exec, kill_request, lease_sandbox, run_request, started};
use serde_json::json;
use trogon_sandbox_contract::{ExecutionId, KillCommandResponse, PollCommandResponse};
use trogon_sandbox_e2b_worker::{E2bExec, LeaseSandbox};

const LEASE: &str = "native-cancellation";

async fn launch(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    guest: &Guest,
    command: &[&str],
) -> TestResult<(ExecutionId, Binding)> {
    let request = run_request(json!({
        "lease_id": LEASE, "command": command, "detached": true, "deadline": "lease", "run_as_user": "root"
    }))?;
    let response = exec.handle_run_command(sandbox, &request).await;
    Ok((started(&response)?, guest.last_binding()?))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_marker_cannot_hide_owned_descendants_from_exact_native_cancellation() -> TestResult {
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(guest.clone()).await?;
    let observed = async {
        let sandbox = lease_sandbox(&envd.url, "owned-cancellation-fixture")?;
        let exec = exec()?;
        let (sentinel_id, sentinel) = launch(&exec, &sandbox, &guest, &["sleep", "60"]).await?;
        let (target_id, target) = launch(
            &exec,
            &sandbox,
            &guest,
            &["bash", "-c", "sleep 60 >/dev/null 2>&1 & exit 0"],
        )
        .await?;
        let marker = format!("/tmp/.trogon-exec-{}/exit", target_id.as_str());
        let exit = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if let Ok(bytes) = tokio::fs::read(&marker).await {
                    break bytes;
                }
            }
        })
        .await?;
        let before = guest.group_members(target)?;
        let before_poll = exec.handle_poll_command(&sandbox, LEASE, target_id.as_str()).await;
        let signaled = exec
            .handle_kill_command(&sandbox, LEASE, &kill_request(LEASE, target_id.clone()))
            .await;
        let remaining = guest.group_members(target)?;
        let unrelated = guest.group_members(sentinel)?;
        let terminal = exec.handle_poll_command(&sandbox, LEASE, target_id.as_str()).await;
        let sentinel_state = exec.handle_poll_command(&sandbox, LEASE, sentinel_id.as_str()).await;
        Ok::<_, Boxed>((
            exit,
            target,
            before,
            before_poll,
            signaled,
            remaining,
            unrelated,
            terminal,
            sentinel_state,
        ))
    }
    .await;
    envd.shutdown().await;
    guest.cleanup()?;
    let (exit, target, before, before_poll, signaled, remaining, unrelated, terminal, sentinel_state) = observed?;
    assert_eq!(
        exit, b"0\n",
        "the requested command must have completed naturally before cancellation"
    );
    assert!(
        before.contains(&target.pid) && before.len() >= 2,
        "an exited requested command must leave a real owned descendant alive: {before:?}"
    );
    assert!(
        matches!(before_poll, PollCommandResponse::Running),
        "an exit sidefile must not establish completion while the owned native group still runs: {before_poll:?}"
    );
    assert!(
        signaled == KillCommandResponse::Signaled && remaining.is_empty(),
        "an exit sidefile must not bypass exact group termination: response={signaled:?}, remaining={remaining:?}"
    );
    assert!(
        !unrelated.is_empty() && matches!(sentinel_state, PollCommandResponse::Running),
        "exact cancellation must preserve the unrelated owned sentinel: group={unrelated:?}, state={sentinel_state:?}"
    );
    assert!(
        matches!(terminal, PollCommandResponse::Completed(ref output) if output.exit_code == 0 && !output.timed_out.is_timed_out()),
        "native termination must preserve the already-settled command output: {terminal:?}"
    );
    Ok(())
}
