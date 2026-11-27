//! Detached commands against a guest that runs them on this machine.

mod guest_support;

use std::time::Duration;

use guest_support::*;
use serde_json::json;
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, ExecutionId, KillCommandRequest, KillCommandResponse, OutputTruncation,
    PollCommandResponse, UnixSignal,
};

fn detached(change: impl FnOnce(&mut serde_json::Value)) -> impl FnOnce(&mut serde_json::Value) {
    move |wire| {
        wire["detached"] = json!(true);
        change(wire);
    }
}

fn kill(execution: &str) -> KillCommandRequest {
    KillCommandRequest {
        lease_id: lease_id(),
        execution_id: ExecutionId::new(execution),
        signal: UnixSignal::Term,
    }
}

#[tokio::test]
async fn a_detached_command_starts_polls_and_is_cleaned_up() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["sh", "-c", "echo out; echo err >&2; exit 4"], detached(|_| {}))?;
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 4);
    assert_eq!(output.stdout, b"out\n");
    assert_eq!(output.stderr, b"err\n");
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(guest.session_deletes(), 1);
    for path in guest.deleted_files() {
        assert!(!std::path::Path::new(&path).exists(), "{path}");
    }
    assert_eq!(guest.calls("POST", "/process/session").len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_command_that_has_not_ended_is_running() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["sleep", "2"], detached(|_| {}))?).await?;
    assert!(matches!(
        exec.handle_poll_command(&guest.sandbox(), LEASE, &id).await,
        PollCommandResponse::Running
    ));
    within(finish(&guest, &exec, &id)).await??;
    Ok(())
}

#[tokio::test]
async fn a_finished_result_is_served_again_from_memory() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["echo", "once"], detached(|_| {}))?).await?;
    let first = within(finish(&guest, &exec, &id)).await??;
    let reads = guest.calls("GET", "/files/download").len();
    let second = finish(&guest, &exec, &id).await?;
    assert_eq!(first, second);
    assert_eq!(guest.calls("GET", "/files/download").len(), reads);
    assert_eq!(guest.session_deletes(), 1);
    Ok(())
}

#[tokio::test]
async fn the_command_deadline_ends_a_detached_command() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["sleep", "30"], detached(|wire| wire["timeout_seconds"] = json!(1)))?;
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
    Ok(())
}

#[tokio::test]
async fn a_lease_deadline_leaves_the_command_running_past_its_timeout() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["sh", "-c", "sleep 3; echo late"],
        detached(|wire| {
            wire["timeout_seconds"] = json!(1);
            wire["deadline"] = json!("lease");
        }),
    )?;
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(output.stdout, b"late\n");
    assert!(
        guest
            .session_commands()
            .iter()
            .all(|command| !command.contains("timeout -k"))
    );
    Ok(())
}

#[tokio::test]
async fn the_output_cap_applies_to_a_detached_command() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["sh", "-c", "head -c 5000 /dev/zero | tr '\\0' x"],
        detached(|wire| wire["max_output_bytes"] = json!(100)),
    )?;
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert!(output.stdout.len() < 1000 && output.stdout.starts_with(b"xxxx"));
    assert_eq!(output.truncated, OutputTruncation::Truncated);
    Ok(())
}

#[tokio::test]
async fn binary_output_survives_a_detached_run() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["sh", "-c", "printf 'a\\000b\\377'"], detached(|_| {}))?;
    let id = start(&guest, &exec, &request).await?;
    assert_eq!(within(finish(&guest, &exec, &id)).await??.stdout, b"a\0b\xff");
    Ok(())
}

#[tokio::test]
async fn a_receipt_that_fails_to_read_keeps_the_execution_for_a_retry() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["echo", "x"], detached(|_| {}))?).await?;
    guest.settle().await;
    guest.fail_downloads(".status", 500, 1).await;
    let mut denied = false;
    for _ in 0..100 {
        match exec.handle_poll_command(&guest.sandbox(), LEASE, &id).await {
            PollCommandResponse::Denied(_) => {
                denied = true;
                break;
            }
            PollCommandResponse::Running => tokio::time::sleep(Duration::from_millis(50)).await,
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    assert!(denied);
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.stdout, b"x\n");
    Ok(())
}

#[tokio::test]
async fn a_stream_that_fails_to_read_keeps_the_execution_for_a_retry() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["echo", "x"], detached(|_| {}))?).await?;
    guest.settle().await;
    guest.fail_downloads(".err", 500, 1).await;
    let mut denied = false;
    for _ in 0..100 {
        match exec.handle_poll_command(&guest.sandbox(), LEASE, &id).await {
            PollCommandResponse::Denied(_) => {
                denied = true;
                break;
            }
            PollCommandResponse::Running => tokio::time::sleep(Duration::from_millis(50)).await,
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    assert!(denied);
    assert_eq!(within(finish(&guest, &exec, &id)).await??.stdout, b"x\n");
    Ok(())
}

#[tokio::test]
async fn what_this_lease_did_not_start_is_not_found() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["sleep", "1"], detached(|_| {}))?).await?;
    assert!(matches!(
        exec.handle_poll_command(&guest.sandbox(), "other", &id).await,
        PollCommandResponse::NotFound
    ));
    assert!(matches!(
        exec.handle_poll_command(
            &trogon_sandbox_daytona_worker::LeaseSandbox {
                id: "elsewhere".to_owned()
            },
            LEASE,
            &id
        )
        .await,
        PollCommandResponse::NotFound
    ));
    assert!(matches!(
        exec.handle_kill_command(&guest.sandbox(), LEASE, &kill("nope")).await,
        KillCommandResponse::NotFound
    ));
    exec.forget_lease(LEASE).await;
    assert!(matches!(
        exec.handle_poll_command(&guest.sandbox(), LEASE, &id).await,
        PollCommandResponse::NotFound
    ));
    guest.settle().await;
    Ok(())
}

#[tokio::test]
async fn an_expired_result_is_dropped_when_the_next_command_starts() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?.with_result_ttl(Duration::ZERO);
    let first = start(&guest, &exec, &run_request(&["echo", "a"], detached(|_| {}))?).await?;
    within(finish(&guest, &exec, &first)).await??;
    tokio::time::sleep(Duration::from_millis(5)).await;
    start(&guest, &exec, &run_request(&["echo", "b"], detached(|_| {}))?).await?;
    assert!(matches!(
        exec.handle_poll_command(&guest.sandbox(), LEASE, &first).await,
        PollCommandResponse::NotFound
    ));
    guest.settle().await;
    Ok(())
}

#[tokio::test]
async fn killing_a_finished_command_reports_it_completed() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["echo", "x"], detached(|_| {}))?).await?;
    within(finish(&guest, &exec, &id)).await??;
    assert!(matches!(
        exec.handle_kill_command(&guest.sandbox(), LEASE, &kill(&id)).await,
        KillCommandResponse::AlreadyCompleted
    ));
    Ok(())
}

#[tokio::test]
async fn killing_a_running_command_without_a_runtime_is_denied() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["sleep", "2"], detached(|_| {}))?).await?;
    assert!(matches!(
        exec.handle_kill_command(&guest.sandbox(), LEASE, &kill(&id)).await,
        KillCommandResponse::Denied(_)
    ));
    within(finish(&guest, &exec, &id)).await??;
    Ok(())
}

#[tokio::test]
async fn a_shell_that_cannot_enter_its_directory_exits_nonzero() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["echo", "x"],
        detached(|wire| wire["cwd"] = json!("/definitely/not/here")),
    )?;
    // The shell starts, fails in `cd`, and leaves a nonzero exit; the run itself is still accepted.
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_ne!(output.exit_code, 0);
    Ok(())
}

#[tokio::test]
async fn a_forgotten_session_record_neither_stops_the_command_nor_loses_its_result() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["sh", "-c", "sleep 1; printf known; printf noted >&2; exit 37"],
        detached(|_| {}),
    )?;
    let id = start(&guest, &exec, &request).await?;
    guest.lose_sessions().await;
    assert!(guest.session_running().await);
    let first = within(finish(&guest, &exec, &id)).await??;
    let second = finish(&guest, &exec, &id).await?;
    for output in [&first, &second] {
        assert_eq!(output.exit_code, 37);
        assert_eq!(output.stdout, b"known");
        assert_eq!(output.stderr, b"noted");
        assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    }
    Ok(())
}

#[tokio::test]
async fn forgetting_the_session_does_not_stand_in_for_the_deadline() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["sleep", "30"], detached(|wire| wire["timeout_seconds"] = json!(1)))?;
    let id = start(&guest, &exec, &request).await?;
    guest.lose_sessions().await;
    assert!(guest.session_running().await);
    assert!(within(guest.settle()).await?);
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
    Ok(())
}

#[tokio::test]
async fn a_zero_command_budget_does_not_bound_a_lease_deadline() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["sh", "-c", "sleep 2; echo done"],
        detached(|wire| {
            wire["timeout_seconds"] = json!(0);
            wire["deadline"] = json!("lease");
        }),
    )?;
    let id = start(&guest, &exec, &request).await?;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(guest.session_running().await);
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"done\n");
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    Ok(())
}

#[tokio::test]
async fn a_lease_deadline_keeps_the_requested_output_cap() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["sh", "-c", "head -c 5000 /dev/zero | tr '\\0' y"],
        detached(|wire| {
            wire["max_output_bytes"] = json!(100);
            wire["deadline"] = json!("lease");
        }),
    )?;
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert!(output.stdout.len() < 1000 && output.stdout.starts_with(b"yyyy"));
    assert_eq!(output.truncated, OutputTruncation::Truncated);
    Ok(())
}

#[tokio::test]
async fn the_default_cap_keeps_small_output_whole() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["sh", "-c", "echo small; echo note >&2; exit 7"], detached(|_| {}))?;
    let id = start(&guest, &exec, &request).await?;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, b"small\n");
    assert_eq!(output.stderr, b"note\n");
    assert_eq!(output.truncated, OutputTruncation::Complete);
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    Ok(())
}

#[tokio::test]
async fn a_capped_result_stays_capped_after_a_receipt_retry_and_from_memory() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(
        &["sh", "-c", "head -c 5000 /dev/zero | tr '\\0' z"],
        detached(|wire| wire["max_output_bytes"] = json!(100)),
    )?;
    let id = start(&guest, &exec, &request).await?;
    guest.settle().await;
    guest.fail_downloads(".status", 500, 1).await;
    let mut denied = false;
    for _ in 0..100 {
        match exec.handle_poll_command(&guest.sandbox(), LEASE, &id).await {
            PollCommandResponse::Denied(_) => {
                denied = true;
                break;
            }
            PollCommandResponse::Running => tokio::time::sleep(Duration::from_millis(50)).await,
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    assert!(denied);
    let first = within(finish(&guest, &exec, &id)).await??;
    let second = finish(&guest, &exec, &id).await?;
    for output in [&first, &second] {
        assert!(output.stdout.len() < 1000 && output.stdout.starts_with(b"zzzz"));
        assert_eq!(output.truncated, OutputTruncation::Truncated);
    }
    assert_eq!(first, second);
    Ok(())
}
