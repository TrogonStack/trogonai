use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_export_is_answered_as_unsupported() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let worker_id = WorkerId::new("worker-1")?;
    let cancel = CancellationToken::new();
    serve_snapshot_export(&client, &worker_id, &cancel).await?;
    client.flush().await?;
    let subject = subjects::worker_snapshot_export(&worker_id);
    let reply = tokio::time::timeout(Duration::from_secs(10), client.request(subject.clone(), "{}".into())).await??;
    let outcome: SnapshotExportOutcome = serde_json::from_slice(&reply.payload)?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { message }) if message == SNAPSHOT_EXPORT_UNSUPPORTED
    ));
    client.publish(subject.clone(), "{}".into()).await?;
    let again = tokio::time::timeout(Duration::from_secs(10), client.request(subject, "{}".into())).await??;
    assert!(serde_json::from_slice::<Value>(&again.payload)?.is_object());
    cancel.cancel();
    Ok(())
}
