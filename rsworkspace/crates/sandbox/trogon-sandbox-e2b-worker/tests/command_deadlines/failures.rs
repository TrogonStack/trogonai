use super::*;
use trogon_sandbox_contract::LeaseError;

async fn prove_retained_stop_failure(accepted_but_running: bool) -> TestResult {
    let (server, process, exec, sandbox) = fixture().await?;
    process.fail_kills.store(!accepted_but_running, Ordering::SeqCst);
    process
        .keep_running_after_kill
        .store(accepted_but_running, Ordering::SeqCst);
    let response = exec
        .handle_run_command(&sandbox, &request(CommandDeadline::Timeout)?)
        .await;
    let execution = started(&response)?;
    let attempted = tokio::time::timeout(Duration::from_secs(4), process.kill_attempted.notified())
        .await
        .is_ok();
    let still_running = process.active.load(Ordering::SeqCst);
    let unconfirmed = exec
        .handle_poll_command(&sandbox, "deadline-lease", execution.as_str())
        .await;
    process.fail_kills.store(false, Ordering::SeqCst);
    process.keep_running_after_kill.store(false, Ordering::SeqCst);
    let retried = tokio::time::timeout(Duration::from_secs(4), process.stopped.notified())
        .await
        .is_ok();
    let confirmed = exec
        .handle_poll_command(&sandbox, "deadline-lease", execution.as_str())
        .await;
    exec.handle_kill_command(&sandbox, "deadline-lease", &kill_request("deadline-lease", execution))
        .await;
    let disposed = !process.active.load(Ordering::SeqCst);
    server.shutdown().await;
    assert!(disposed, "owned native fixture must be disposed before assertion");
    assert!(
        attempted && still_running && matches!(unconfirmed, PollCommandResponse::Denied(LeaseError::Backend { .. })),
        "a failed/unconfirmed stop must preserve command custody and uncertainty: accepted_but_running={accepted_but_running}, attempted={attempted}, still_running={still_running}, response={unconfirmed:?}"
    );
    assert!(
        retried
            && matches!(confirmed, PollCommandResponse::Completed(output) if output.timed_out.is_timed_out() && output.exit_code == -1),
        "the retained supervisor must retry and require actual native end before timeout completion"
    );
    Ok(())
}

#[tokio::test]
async fn failed_native_deadline_kill_retains_uncertainty_and_retries_exact_process() -> TestResult {
    prove_retained_stop_failure(false).await
}

#[tokio::test]
async fn accepted_native_deadline_kill_without_termination_is_not_timeout_completion() -> TestResult {
    prove_retained_stop_failure(true).await
}
