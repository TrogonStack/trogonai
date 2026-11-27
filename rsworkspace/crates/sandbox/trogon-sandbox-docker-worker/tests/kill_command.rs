//! Kill-command (Unix signal delivery to a detached exec) integration tests.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker kill_command
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request, wait_completed};
use trogon_sandbox_protocol::Provider as _;

use std::time::Duration;

use trogon_sandbox_contract::{
    BootMode, CommandDeadline, CommandEnv, ExecutionId, KillCommandResponse, LeaseId, LeaseRequest, OutputRetention,
    PollCommandResponse, StdinMode, StreamedOutput, UnixSignal,
};

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("kill-command-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_command_signals_running_process_and_it_completes() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec!["sleep".to_string(), "300".to_string()],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    // Give the exec a moment to actually start before killing it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let first_poll = provider
        .poll_command_for_lease(&lease_id, &execution_id)
        .await
        .ok_or("the execution is tracked")?;
    assert!(
        matches!(first_poll, PollCommandResponse::Running),
        "expected Running before kill_command, got {first_poll:?}"
    );

    let kill_response = provider
        .kill_command_for_lease(&lease_id, &execution_id, UnixSignal::Kill)
        .await;
    assert!(
        matches!(kill_response, KillCommandResponse::Signaled),
        "expected Signaled, got {kill_response:?}"
    );

    let completed = wait_completed(&provider, &lease_id, &execution_id, 10).await?;

    assert_ne!(
        completed.exit_code, 0,
        "expected a non-zero exit code after SIGKILL, got {completed:?}"
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_command_returns_already_completed_for_finished_execution() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec!["true".to_string()],
            None,
            &CommandEnv::default(),
            30,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let _completed = wait_completed(&provider, &lease_id, &execution_id, 10).await?;

    let kill_response = provider
        .kill_command_for_lease(&lease_id, &execution_id, UnixSignal::Term)
        .await;
    assert!(
        matches!(kill_response, KillCommandResponse::AlreadyCompleted),
        "expected AlreadyCompleted, got {kill_response:?}"
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_command_returns_not_found_for_unknown_execution_id() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let lease_id = LeaseId::new("no-such-lease");
    let result = provider
        .kill_command_for_lease(&lease_id, &ExecutionId::new("no-such-execution-id"), UnixSignal::Term)
        .await;
    assert!(matches!(result, KillCommandResponse::NotFound));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_command_also_kills_child_processes() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "sleep 301 & sleep 302 & wait".to_string(),
            ],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    tokio::time::sleep(Duration::from_millis(700)).await;

    let count_children = || async {
        let output = provider
            .run_command_for_lease(
                &lease_id,
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "for c in /proc/[0-9]*/cmdline; do tr '\\0' ' ' < $c 2>/dev/null; echo; done | grep -c '^sleep 30[12] *$'; true".to_string(),
                ],
                None,
                &CommandEnv::default(),
                10,
                None,
                StreamedOutput::Disabled,
                usize::MAX,
            )
            .await
            ?;
        Ok::<String, anyhow::Error>(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    assert_eq!(count_children().await?, "2", "children should be running");

    let kill_response = provider
        .kill_command_for_lease(&lease_id, &execution_id, UnixSignal::Kill)
        .await;
    assert!(
        matches!(kill_response, KillCommandResponse::Signaled),
        "expected Signaled, got {kill_response:?}"
    );

    wait_completed(&provider, &lease_id, &execution_id, 10).await?;

    assert_eq!(count_children().await?, "0", "children should be killed too");

    let second = provider
        .kill_command_for_lease(&lease_id, &execution_id, UnixSignal::Kill)
        .await;
    assert!(
        matches!(second, KillCommandResponse::AlreadyCompleted),
        "expected AlreadyCompleted, got {second:?}"
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_deadline_command_outlives_timeout_seconds_until_killed() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec!["sleep".to_string(), "300".to_string()],
            None,
            &CommandEnv::default(),
            2,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Lease,
            usize::MAX,
            None,
        )
        .await?;

    tokio::time::sleep(Duration::from_secs(4)).await;

    let poll = provider
        .poll_command_for_lease(&lease_id, &execution_id)
        .await
        .ok_or("the execution is tracked")?;
    assert!(
        matches!(poll, PollCommandResponse::Running),
        "lease deadline command must outlive timeout_seconds, got {poll:?}"
    );

    let kill_response = provider
        .kill_command_for_lease(&lease_id, &execution_id, UnixSignal::Kill)
        .await;
    assert!(
        matches!(kill_response, KillCommandResponse::Signaled),
        "expected Signaled, got {kill_response:?}"
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn synchronous_timeout_kills_the_exec_and_reports_timed_out() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let output = provider
        .run_command_for_lease(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "sleep 303 & sleep 304 & wait".to_string(),
            ],
            None,
            &CommandEnv::default(),
            1,
            None,
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(output.exit_code, -1);
    assert!(output.timed_out.is_timed_out(), "got {output:?}");

    tokio::time::sleep(Duration::from_millis(500)).await;
    let survivors = provider
        .run_command_for_lease(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "for c in /proc/[0-9]*/cmdline; do tr '\\0' ' ' < $c 2>/dev/null; echo; done | grep -c '^sleep 30[34] *$'; true".to_string(),
            ],
            None,
            &CommandEnv::default(),
            10,
            None,
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await
        ?;
    assert_eq!(
        String::from_utf8_lossy(&survivors.stdout).trim(),
        "0",
        "the timed-out exec's process tree must be killed"
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn synchronous_output_over_the_cap_is_truncated() -> TestResult {
    let provider = build_provider("kill_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let output = provider
        .run_command_for_lease(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf HEAD; head -c 100000 /dev/zero | tr '\\0' x; printf TAIL".to_string(),
            ],
            None,
            &CommandEnv::default(),
            30,
            None,
            StreamedOutput::Disabled,
            1024,
        )
        .await?;
    assert_eq!(output.exit_code, 0);
    assert!(output.truncated.is_truncated());
    assert!(output.stdout.starts_with(b"HEAD"));
    assert!(output.stdout.ends_with(b"TAIL"));
    assert!(output.stdout.len() < 1024 + 64, "got {}", output.stdout.len());

    provider.cancel(&lease_id).await?;
    Ok(())
}
