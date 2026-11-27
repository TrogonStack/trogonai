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
