use super::*;
use std::sync::atomic::Ordering;

#[tokio::test(start_paused = true)]
async fn a_command_past_its_deadline_is_killed_by_marker_and_reported_timed_out() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let remote = Arc::new(RemoteExecLifetime::default());
    *mock.remote_exec.locked() = Some(remote.clone());
    mock.exec_hangs.store(true, Ordering::SeqCst);
    let output = provider(mock.clone())
        .exec_in_pod_bounded(
            "pod",
            vec!["sleep".to_string(), "100".to_string()],
            "SANDBOX_EXECUTION_ID=abc",
            std::time::Duration::from_secs(3),
            1024,
        )
        .await?;
    let task = remote.task.locked().take().ok_or("missing value")?;
    task.await?;

    assert_eq!(output.exit_code, -1);
    assert!(output.timed_out);
    assert_eq!(output.stderr, b"command timed out after 3s");

    let calls = mock.exec_calls.locked();
    let kill = calls.last().ok_or("missing value")?;
    assert!(kill[2].contains("kill"), "{kill:?}");
    assert_eq!(kill[4], "SANDBOX_EXECUTION_ID=abc");
    assert_eq!(kill[5], "KILL");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_command_that_finishes_in_time_is_not_signaled() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let output = provider(mock.clone())
        .exec_in_pod_bounded(
            "pod",
            vec!["true".to_string()],
            "SANDBOX_EXECUTION_ID=abc",
            std::time::Duration::from_secs(3),
            1024,
        )
        .await?;

    assert!(!output.timed_out);
    assert_eq!(mock.exec_calls.locked().len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn deadline_cannot_report_completion_when_marker_kill_exec_fails() -> TestResult {
    deadline_requires_confirmed_signal_or_an_explicit_error(false).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn deadline_cannot_report_completion_when_marker_kill_never_confirms() -> TestResult {
    deadline_requires_confirmed_signal_or_an_explicit_error(true).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn accepted_marker_signal_requires_native_completion_before_timeout_success() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let remote = Arc::new(RemoteExecLifetime::default());
    *mock.remote_exec.locked() = Some(remote.clone());
    mock.exec_hangs.store(true, Ordering::SeqCst);
    mock.signal_keeps_exec_running.store(true, Ordering::SeqCst);
    let result = provider(mock)
        .exec_in_pod_bounded(
            "pod",
            vec!["held-native-command".into()],
            "SANDBOX_EXECUTION_ID=accepted-marker",
            Duration::from_secs(3),
            1024,
        )
        .await;
    let still_running = remote.running.load(Ordering::SeqCst);
    let accepted_signals = remote.signal_attempts.load(Ordering::SeqCst);
    remote.release.cancel();
    let task = remote.task.locked().take().ok_or("missing value")?;
    task.await?;
    assert!(still_running && accepted_signals == 1);
    assert!(
        result.is_err(),
        "accepted marker signaling cannot prove the original native command finished: {result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn signal_by_marker_reports_whether_a_process_matched() -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let p = provider(mock.clone());
    assert!(
        p.signal_exec_by_marker("pod", "SANDBOX_EXECUTION_ID=abc", UnixSignal::Term)
            .await
    );
    assert_eq!(
        mock.exec_calls.locked()[0][5],
        "TERM",
        "the signal name is passed to the script"
    );
    Ok(())
}
