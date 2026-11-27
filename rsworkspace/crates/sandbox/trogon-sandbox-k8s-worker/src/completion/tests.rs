use super::*;

use trogon_sandbox_contract::LeaseId;

use crate::PollOutcome;
use crate::test_fixtures::TestResult;

const LIMIT: Duration = Duration::from_secs(2);

fn output(stderr: &[u8]) -> PodExecOutput {
    PodExecOutput {
        exit_code: 7,
        stderr: stderr.to_vec(),
        ..PodExecOutput::default()
    }
}

fn finishing_after(delay: Duration, result: Result<PodExecOutput, K8sWorkerError>) -> Completion {
    Box::pin(async move {
        tokio::time::sleep(delay).await;
        result
    })
}

async fn tracked(tracker: &ExecutionTracker, execution_id: &str) -> Option<(ExecutionTracker, String)> {
    tracker
        .spawn(LeaseId::new("lease-1"), execution_id.to_owned(), || async {
            std::future::pending::<Result<PodExecOutput, K8sWorkerError>>().await
        })
        .await;
    Some((tracker.clone(), execution_id.to_owned()))
}

async fn kill_succeeds() -> Result<(), K8sWorkerError> {
    Ok(())
}

async fn kill_fails() -> Result<(), K8sWorkerError> {
    Err(K8sWorkerError::TerminationUnconfirmed)
}

#[test]
fn the_marker_names_the_execution() {
    assert!(execution_marker("e-1").ends_with("=e-1"));
}

#[test]
fn a_timed_out_result_keeps_what_the_command_wrote() {
    let result = timed_out_result(output(b"partial"), Duration::from_secs(9));
    assert_eq!(
        (result.exit_code, result.timed_out, result.stderr.as_slice()),
        (-1, true, &b"partial"[..])
    );
}

#[test]
fn a_timed_out_result_without_output_says_so() {
    let result = timed_out_result(output(b""), Duration::from_secs(9));
    assert_eq!(result.stderr, b"command timed out after 9s");
}

#[tokio::test(start_paused = true)]
async fn a_command_that_finishes_in_time_is_returned_untouched() -> TestResult {
    let bound = bound_completion(
        finishing_after(Duration::from_secs(1), Ok(output(b""))),
        LIMIT,
        kill_fails,
        None,
    );
    assert_eq!(bound.await?, output(b""));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_killed_command_that_settles_is_reported_as_timed_out() -> TestResult {
    let bound = bound_completion(
        finishing_after(Duration::from_secs(3), Ok(output(b""))),
        LIMIT,
        kill_succeeds,
        None,
    );
    let result = bound.await?;
    assert!(result.timed_out);
    assert_eq!(result.exit_code, -1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_killed_command_whose_completion_fails_is_uncertain() -> TestResult {
    let tracker = ExecutionTracker::default();
    let retained = tracked(&tracker, "e-1").await;
    let completion = finishing_after(Duration::from_secs(3), Err(K8sWorkerError::CompletionUnconfirmed));
    let bound = bound_completion(completion, LIMIT, kill_succeeds, retained);
    assert!(matches!(bound.await, Err(K8sWorkerError::CompletionUnconfirmed)));
    assert!(matches!(tracker.poll("e-1").await, PollOutcome::Failed(_)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_kill_with_nothing_to_keep_is_an_error() {
    let bound = bound_completion(
        finishing_after(Duration::from_secs(60), Ok(output(b""))),
        LIMIT,
        kill_fails,
        None,
    );
    assert!(matches!(bound.await, Err(K8sWorkerError::TerminationUnconfirmed)));
}

#[tokio::test(start_paused = true)]
async fn a_failed_kill_keeps_waiting_for_the_original_completion() -> TestResult {
    let tracker = ExecutionTracker::default();
    let retained = tracked(&tracker, "e-1").await;
    let bound = bound_completion(
        finishing_after(Duration::from_secs(30), Ok(output(b"late"))),
        LIMIT,
        kill_fails,
        retained,
    );
    assert_eq!(bound.await?, output(b"late"));
    assert!(matches!(tracker.poll("e-1").await, PollOutcome::Failed(_)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_completion_that_never_settles_without_an_owner_is_an_error() {
    let bound = bound_completion(
        finishing_after(Duration::from_secs(600), Ok(output(b""))),
        LIMIT,
        kill_succeeds,
        None,
    );
    assert!(matches!(bound.await, Err(K8sWorkerError::CompletionUnconfirmed)));
}

#[tokio::test(start_paused = true)]
async fn a_completion_that_settles_late_is_reported_as_timed_out() -> TestResult {
    let tracker = ExecutionTracker::default();
    let retained = tracked(&tracker, "e-1").await;
    let bound = bound_completion(
        finishing_after(Duration::from_secs(600), Ok(output(b""))),
        LIMIT,
        kill_succeeds,
        retained,
    );
    assert!(bound.await?.timed_out);
    assert!(matches!(tracker.poll("e-1").await, PollOutcome::Failed(_)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_execution_nobody_tracks_cannot_be_marked_uncertain() {
    let tracker = ExecutionTracker::default();
    let retained = Some((tracker, "ghost".to_owned()));
    let bound = bound_completion(
        finishing_after(Duration::from_secs(60), Ok(output(b""))),
        LIMIT,
        kill_fails,
        retained,
    );
    assert!(matches!(bound.await, Err(K8sWorkerError::TerminationUnconfirmed)));
}
