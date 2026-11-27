//! Detached (non-blocking) command execution and polling.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker detached_command
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request, wait_completed};

use trogon_sandbox_contract::{
    BootMode, CommandDeadline, CommandEnv, ExecutionId, LeaseId, LeaseRequest, OutputRetention, PollCommandResponse,
    StdinMode, StreamedOutput,
};
use trogon_sandbox_protocol::Provider;

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("detached-command-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_command_reports_running_then_completed_with_output() -> TestResult {
    let provider = build_provider("detached_command").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "sleep 1 && echo hello-from-detached-exec".to_string(),
            ],
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

    // Poll immediately: the sleep hasn't elapsed yet, so this should observe
    // the command still running at least once.
    let first_poll = provider
        .poll_command_for_lease(&lease_id, &execution_id)
        .await
        .ok_or("the execution is tracked")?;
    assert!(
        matches!(first_poll, PollCommandResponse::Running),
        "expected Running immediately after starting a sleeping command, got {first_poll:?}"
    );

    let completed = wait_completed(&provider, &lease_id, &execution_id, 10).await?;

    assert_eq!(completed.exit_code, 0);
    assert!(
        String::from_utf8_lossy(&completed.stdout).contains("hello-from-detached-exec"),
        "stdout: {:?}",
        String::from_utf8_lossy(&completed.stdout)
    );

    // Polling again after completion must keep returning the same output
    // rather than losing it or reporting NotFound.
    let repeat_poll = provider
        .poll_command_for_lease(&lease_id, &execution_id)
        .await
        .ok_or("the execution is tracked")?;
    assert!(matches!(repeat_poll, PollCommandResponse::Completed(o) if o == completed));

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_command_returns_not_found_for_unknown_execution_id() -> TestResult {
    let provider = build_provider("detached_command").await?;

    let lease_id = LeaseId::new("no-such-lease");
    let result = provider
        .poll_command_for_lease(&lease_id, &ExecutionId::new("no-such-execution-id"))
        .await;
    assert!(result.is_none());
    Ok(())
}
