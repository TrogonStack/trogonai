//! Signals to detached commands that run under the pinned supervisor.
//!
//! The lanes that start a real supervisor need a Linux host and `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_ARTIFACT` with
//! `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_SHA256`; without them they return at once.

mod guest_support;

use std::time::Duration;

use guest_support::*;
use serde_json::json;
use trogon_sandbox_contract::{ExecutionId, KillCommandRequest, KillCommandResponse, LeaseId, UnixSignal};

fn detached(wire: &mut serde_json::Value) {
    wire["detached"] = json!(true);
}

fn kill_request(execution: &str, signal: UnixSignal) -> KillCommandRequest {
    KillCommandRequest {
        lease_id: LeaseId::new(LEASE),
        execution_id: ExecutionId::new(execution),
        signal,
    }
}

/// Waits until the command has written `needle` to its stdout.
async fn wait_for_output(guest: &Guest, execution: &str, needle: &str) -> TestResult {
    for _ in 0..200 {
        let (_, output) = guest
            .shell(&format!("cat /tmp/.trogon-exec-{execution}.out 2>/dev/null"))
            .await?;
        if output.contains(needle) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("the command never printed {needle}").into())
}

#[tokio::test]
async fn without_a_runtime_a_detached_command_cannot_be_signaled() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["sleep", "30"], detached)?).await?;
    let response = exec
        .handle_kill_command(&guest.sandbox(), LEASE, &kill_request(&id, UnixSignal::Term))
        .await;
    assert!(matches!(response, KillCommandResponse::Denied(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn a_signal_to_an_unknown_execution_is_not_found() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let response = exec
        .handle_kill_command(&guest.sandbox(), LEASE, &kill_request("missing", UnixSignal::Term))
        .await;
    assert!(matches!(response, KillCommandResponse::NotFound), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn term_is_delivered_and_the_real_exit_and_bytes_are_retained() -> TestResult {
    let guest = Guest::start().await?;
    let Some(exec) = guest.exec_with_supervisor()? else {
        return Ok(());
    };
    let script = "trap 'echo caught; echo oops >&2; exit 7' TERM; echo ready; while :; do sleep 0.1; done";
    let id = start(&guest, &exec, &run_request(&["/bin/sh", "-c", script], detached)?).await?;
    within(wait_for_output(&guest, &id, "ready")).await??;
    let response = exec
        .handle_kill_command(&guest.sandbox(), LEASE, &kill_request(&id, UnixSignal::Term))
        .await;
    assert!(matches!(response, KillCommandResponse::Signaled), "{response:?}");
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, b"ready\ncaught\n");
    assert_eq!(output.stderr, b"oops\n");
    assert!(!output.timed_out.is_timed_out());
    Ok(())
}

#[tokio::test]
async fn kill_stops_the_command_and_retains_the_signal_exit() -> TestResult {
    let guest = Guest::start().await?;
    let Some(exec) = guest.exec_with_supervisor()? else {
        return Ok(());
    };
    let script = "echo started; sleep 60";
    let id = start(&guest, &exec, &run_request(&["/bin/sh", "-c", script], detached)?).await?;
    within(wait_for_output(&guest, &id, "started")).await??;
    let response = exec
        .handle_kill_command(&guest.sandbox(), LEASE, &kill_request(&id, UnixSignal::Kill))
        .await;
    assert!(matches!(response, KillCommandResponse::Signaled), "{response:?}");
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 137);
    assert_eq!(output.stdout, b"started\n");
    let again = exec
        .handle_kill_command(&guest.sandbox(), LEASE, &kill_request(&id, UnixSignal::Kill))
        .await;
    assert!(matches!(again, KillCommandResponse::AlreadyCompleted), "{again:?}");
    Ok(())
}

#[tokio::test]
async fn term_under_a_finite_deadline_waits_for_the_user_exit_without_a_timeout() -> TestResult {
    let guest = Guest::start().await?;
    let Some(exec) = guest.exec_with_supervisor()? else {
        return Ok(());
    };
    let script = "trap 'sleep 0.3; exit 3' TERM; echo ready; while :; do sleep 0.1; done";
    let request = run_request(&["/bin/sh", "-c", script], |wire| {
        detached(wire);
        wire["timeout_seconds"] = json!(30);
        wire["deadline"] = json!("timeout");
    })?;
    let id = start(&guest, &exec, &request).await?;
    within(wait_for_output(&guest, &id, "ready")).await??;
    let response = exec
        .handle_kill_command(&guest.sandbox(), LEASE, &kill_request(&id, UnixSignal::Term))
        .await;
    assert!(matches!(response, KillCommandResponse::Signaled), "{response:?}");
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 3);
    assert!(!output.timed_out.is_timed_out());
    Ok(())
}
