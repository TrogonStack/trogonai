use super::*;
use crate::test_fixtures::TestResult;

fn pending() -> impl Future<Output = Result<PodExecOutput, K8sWorkerError>> + Send + 'static {
    std::future::pending()
}

fn chunk() -> Result<StdinChunk, Box<dyn std::error::Error>> {
    Ok(StdinChunk::new(b"x".to_vec())?)
}

#[tokio::test]
async fn stdin_of_another_leases_execution_is_not_found() -> TestResult {
    let tracker = ExecutionTracker::default();
    tracker.spawn(LeaseId::new("lease-1"), "e-1".to_owned(), pending).await;
    let reply = tracker
        .write_stdin(
            &LeaseId::new("lease-2"),
            "e-1",
            StdinSeq::new(0),
            chunk()?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(reply, WriteStdinResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn stdin_of_an_uncertain_execution_is_closed() -> TestResult {
    let tracker = ExecutionTracker::default();
    tracker.spawn(LeaseId::new("lease-1"), "e-1".to_owned(), pending).await;
    assert!(tracker.mark_uncertain("e-1", "lost".to_owned()).await);
    let reply = tracker
        .write_stdin(
            &LeaseId::new("lease-1"),
            "e-1",
            StdinSeq::new(0),
            chunk()?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(reply, WriteStdinResponse::StdinClosed);
    Ok(())
}

fn ready() -> impl Future<Output = Result<PodExecOutput, K8sWorkerError>> + Send + 'static {
    std::future::ready(Ok(PodExecOutput::default()))
}

async fn finished(tracker: &ExecutionTracker, lease: &LeaseId, id: &str) {
    for _ in 0..200 {
        if tracker.status(lease, id).await == ExecutionStatus::Finished {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn polling_before_completion_reports_running() {
    let tracker = ExecutionTracker::default();
    tracker.spawn(LeaseId::new("lease-1"), "e-1".to_owned(), pending).await;
    assert!(matches!(tracker.poll("e-1").await, PollOutcome::Running));
    assert!(matches!(tracker.poll("e-unknown").await, PollOutcome::NotFound));
}

#[tokio::test]
async fn status_tells_running_finished_and_foreign_executions_apart() {
    let tracker = ExecutionTracker::default();
    let owner = LeaseId::new("lease-1");
    tracker.spawn(owner.clone(), "e-running".to_owned(), pending).await;
    tracker.spawn(owner.clone(), "e-done".to_owned(), ready).await;
    finished(&tracker, &owner, "e-done").await;

    assert_eq!(tracker.status(&owner, "e-running").await, ExecutionStatus::Running);
    assert_eq!(tracker.status(&owner, "e-done").await, ExecutionStatus::Finished);
    assert_eq!(tracker.status(&owner, "e-unknown").await, ExecutionStatus::Unknown);
    let other = LeaseId::new("lease-2");
    assert_eq!(tracker.status(&other, "e-running").await, ExecutionStatus::Unknown);
    assert_eq!(tracker.status(&other, "e-done").await, ExecutionStatus::Unknown);
}

#[tokio::test]
async fn an_execution_with_an_unconfirmed_outcome_still_reads_as_running() {
    let tracker = ExecutionTracker::default();
    let owner = LeaseId::new("lease-1");
    tracker.spawn(owner.clone(), "e-1".to_owned(), pending).await;
    assert!(tracker.mark_uncertain("e-1", "lost".to_owned()).await);
    assert_eq!(tracker.status(&owner, "e-1").await, ExecutionStatus::Running);
    assert!(matches!(tracker.poll("e-1").await, PollOutcome::Failed(message) if message == "lost"));
}
