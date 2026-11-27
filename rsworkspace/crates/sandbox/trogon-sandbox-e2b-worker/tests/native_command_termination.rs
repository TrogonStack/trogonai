#![cfg(target_os = "linux")]

mod native_command_support;
mod native_storage_support;

use std::time::Duration;

use native_command_support::{Binding, Guest, NativeEnvd};
use native_storage_support::{Boxed, TestResult, exec, kill_request, lease_sandbox, run_request, started};
use serde_json::json;
use tokio::time::Instant;
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, KillCommandRequest, KillCommandResponse, PollCommandResponse, UnixSignal,
};
use trogon_sandbox_e2b_worker::{E2bExec, LeaseSandbox};

const LEASE: &str = "native-generic-termination";
const OBSERVE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    OrdinaryTerm,
    HandledTerm,
    Natural,
}

#[derive(Debug)]
struct Observation {
    target: Binding,
    before: Vec<u32>,
    initial_poll: Option<PollCommandResponse>,
    signaled: Option<KillCommandResponse>,
    terminal: PollCommandResponse,
    remaining: Vec<u32>,
    sentinel: Binding,
    sentinel_remaining: Vec<u32>,
    sentinel_poll: PollCommandResponse,
    repeated_kill: KillCommandResponse,
}

async fn launch(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    guest: &Guest,
    command: &[&str],
) -> TestResult<(ExecutionId, Binding)> {
    let request = run_request(json!({"lease_id": LEASE, "command": command, "detached": true, "run_as_user": "root"}))?;
    let response = exec.handle_run_command(sandbox, &request).await;
    Ok((started(&response)?, guest.last_binding()?))
}

async fn running_group(guest: &Guest, binding: Binding) -> TestResult<Vec<u32>> {
    let members = tokio::time::timeout(OBSERVE, async {
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let members = guest.group_members(binding)?;
            if members.contains(&binding.pid) && members.len() >= 2 {
                break Ok::<_, Boxed>(members);
            }
        }
    })
    .await??;
    Ok(members)
}

async fn poll_until_settled(exec: &E2bExec, sandbox: &LeaseSandbox, execution: &ExecutionId) -> PollCommandResponse {
    let until = Instant::now() + OBSERVE;
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let response = exec.handle_poll_command(sandbox, LEASE, execution.as_str()).await;
        if !matches!(response, PollCommandResponse::Running) || Instant::now() >= until {
            return response;
        }
    }
}

fn term_request(execution: ExecutionId) -> KillCommandRequest {
    KillCommandRequest {
        signal: UnixSignal::Term,
        ..kill_request(LEASE, execution)
    }
}

fn completed(response: &PollCommandResponse) -> TestResult<CommandOutput> {
    Ok(serde_json::from_value(serde_json::to_value(response)?)?)
}

async fn signal_running(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    guest: &Guest,
    execution: &ExecutionId,
    target: Binding,
    ready: Option<&std::path::Path>,
) -> TestResult<(Vec<u32>, PollCommandResponse, KillCommandResponse)> {
    let before = running_group(guest, target).await?;
    if let Some(ready) = ready {
        tokio::time::timeout(OBSERVE, async {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if tokio::fs::read(ready).await.is_ok_and(|bytes| bytes == b"ready") {
                    break;
                }
            }
        })
        .await?;
    }
    let initial = exec.handle_poll_command(sandbox, LEASE, execution.as_str()).await;
    let signaled = exec
        .handle_kill_command(sandbox, LEASE, &term_request(execution.clone()))
        .await;
    Ok((before, initial, signaled))
}

async fn observe(case: Case, guest: &Guest, native: &NativeEnvd) -> TestResult<Observation> {
    let sandbox = lease_sandbox(&native.url, "owned-generic-termination-fixture")?;
    let exec = exec()?;
    let (sentinel_id, sentinel) = launch(&exec, &sandbox, guest, &["sleep", "300"]).await?;
    running_group(guest, sentinel).await?;
    let ready = guest.path("native-term-ready");
    let ready_path = ready.to_string_lossy();
    let command = match case {
        Case::OrdinaryTerm => vec!["sleep", "300"],
        Case::HandledTerm => vec![
            "bash",
            "-c",
            "trap 'printf handled; exit 37' TERM; printf ready >\"$1\"; while :; do sleep 30; done",
            "term-handler",
            ready_path.as_ref(),
        ],
        Case::Natural => vec!["bash", "-c", "printf natural; exit 23"],
    };
    let (execution, target) = launch(&exec, &sandbox, guest, &command).await?;
    let (before, initial_poll, signaled) = if case == Case::Natural {
        (Vec::new(), None, None)
    } else {
        let waiting = (case == Case::HandledTerm).then_some(ready.as_path());
        let (before, initial, signaled) = signal_running(&exec, &sandbox, guest, &execution, target, waiting).await?;
        (before, Some(initial), Some(signaled))
    };
    let terminal = poll_until_settled(&exec, &sandbox, &execution).await;
    let remaining = guest.group_members(target)?;
    let sentinel_remaining = guest.group_members(sentinel)?;
    let sentinel_poll = exec.handle_poll_command(&sandbox, LEASE, sentinel_id.as_str()).await;
    let repeated_kill = exec
        .handle_kill_command(&sandbox, LEASE, &term_request(execution))
        .await;
    Ok(Observation {
        target,
        before,
        initial_poll,
        signaled,
        terminal,
        remaining,
        sentinel,
        sentinel_remaining,
        sentinel_poll,
        repeated_kill,
    })
}

async fn prove(case: Case) -> TestResult {
    let guest = Guest::create()?;
    let native = NativeEnvd::start(guest.clone()).await?;
    let observed = observe(case, &guest, &native).await;
    native.shutdown().await;
    guest.cleanup()?;
    let observed = observed?;
    if case != Case::Natural {
        assert!(
            observed.before.contains(&observed.target.pid)
                && observed.before.len() >= 2
                && matches!(observed.initial_poll, Some(PollCommandResponse::Running)),
            "TERM must target the exact running command: {observed:?}"
        );
        assert_eq!(
            observed.signaled,
            Some(KillCommandResponse::Signaled),
            "the public TERM request must report accepted signal delivery: {observed:?}"
        );
    }
    assert!(
        observed.remaining.is_empty(),
        "the exact original command group must have stopped: {observed:?}"
    );
    assert!(
        observed.sentinel_remaining.contains(&observed.sentinel.pid)
            && matches!(observed.sentinel_poll, PollCommandResponse::Running),
        "termination must preserve the unrelated running sentinel: {observed:?}"
    );
    let output = completed(&observed.terminal)?;
    assert!(
        !output.timed_out.is_timed_out(),
        "TERM or natural completion must not become a timeout"
    );
    match case {
        Case::OrdinaryTerm => assert!(output.exit_code != 0, "a killed command must not exit zero"),
        Case::HandledTerm => assert!(
            output.exit_code == 37 && output.stdout == b"handled",
            "the user's TERM handler must retain its actual status and output: {output:?}"
        ),
        Case::Natural => assert!(
            output.exit_code == 23 && output.stdout == b"natural",
            "natural completion must retain its actual status and output: {output:?}"
        ),
    }
    assert!(
        matches!(
            observed.repeated_kill,
            KillCommandResponse::AlreadyCompleted | KillCommandResponse::NotFound
        ),
        "killing the completed command must be settled: {observed:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn term_settles_a_running_command_without_stopping_its_sentinel() -> TestResult {
    prove(Case::OrdinaryTerm).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn term_handler_completion_preserves_its_actual_status_and_output() -> TestResult {
    prove(Case::HandledTerm).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn natural_completion_preserves_its_actual_status_and_output() -> TestResult {
    prove(Case::Natural).await
}
