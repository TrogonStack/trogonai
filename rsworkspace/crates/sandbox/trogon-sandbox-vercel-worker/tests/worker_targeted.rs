//! A worker with a dispatch ledger serves the dispatches and cleanups addressed to it alone, and keeps serving the
//! shared lease subject while legacy dispatches are still sent there.

mod nats_support;
mod support;

use std::time::Duration;

use nats_support::{Running, request_json, until};
use serde_json::{Value, json};
use support::{Boxed, TestResult};
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects;

const ATTEMPT: &str = "attempt-a";

fn target(running: &Running) -> Value {
    json!({"worker_id": running.worker_id.as_str(), "attempt_id": ATTEMPT, "epoch": 1})
}

async fn request(running: &Running, subject: String, body: &Value) -> Result<Value, Boxed> {
    let reply = tokio::time::timeout(
        Duration::from_secs(30),
        running.client.request(subject, serde_json::to_vec(body)?.into()),
    )
    .await??;
    Ok(serde_json::from_slice(&reply.payload)?)
}

async fn dispatch(running: &Running, lease: &str) -> Result<Value, Boxed> {
    let body = json!({"lease_id": lease, "request": request_json(), "target": target(running)});
    request(
        running,
        subjects::worker_dispatch(ProviderKind::Vercel, &running.worker_id),
        &body,
    )
    .await
}

async fn cleanup(running: &Running, lease: &str) -> Result<Value, Boxed> {
    let body = json!({
        "identity": {"lease_id": lease, "provider": "vercel", "target": target(running)},
        "intent": "release",
    });
    request(
        running,
        subjects::worker_cleanup(ProviderKind::Vercel, &running.worker_id),
        &body,
    )
    .await
}

#[tokio::test]
async fn a_targeted_dispatch_is_served_on_the_worker_subject_once_per_attempt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut running = Running::start_with_ledger(Some(&dir.path().join("ledger.sqlite3"))).await?;

    let granted = dispatch(&running, "lease-1").await?;
    let again = dispatch(&running, "lease-1").await?;

    assert_eq!(granted, again);
    assert_eq!(granted["attempt_id"], ATTEMPT, "{granted}");
    assert_eq!(granted["worker_id"], running.worker_id.as_str(), "{granted}");
    assert_eq!(running.fake.creates(), 1);
    running.stop().await
}

/// A cleanup deletes the sandbox its receipt names, so one that is not signed for exactly its assignment is refused
/// and deletes nothing. The signed path is covered against the binary in `targeted_process.rs`.
#[tokio::test]
async fn an_unsigned_targeted_cleanup_is_refused_and_deletes_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut running = Running::start_with_ledger(Some(&dir.path().join("ledger.sqlite3"))).await?;
    dispatch(&running, "lease-1").await?;

    let refused = cleanup(&running, "lease-1").await?;

    assert_eq!(refused["outcome"], "failed", "{refused}");
    assert_eq!(
        refused["message"], "targeted cleanup requires exact verified assignment authority",
        "{refused}"
    );
    assert_eq!(running.fake.deletes(), 0);
    assert_eq!(running.provider.active_lease_count().await, 1);
    running.stop().await
}

#[tokio::test]
async fn a_worker_with_a_ledger_still_serves_legacy_dispatches() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut running = Running::start_with_ledger(Some(&dir.path().join("ledger.sqlite3"))).await?;

    let granted = running.dispatch("legacy-lease").await?;

    assert_eq!(granted["sandbox_id"], "sbx-1", "{granted}");
    assert!(granted.get("attempt_id").is_none(), "{granted}");
    running.stop().await
}

#[tokio::test]
async fn a_worker_without_a_ledger_does_not_answer_on_the_worker_subjects() -> TestResult {
    let mut running = Running::start().await?;
    let body = json!({"lease_id": "lease-1", "request": request_json(), "target": target(&running)});

    running
        .silent(
            &subjects::worker_dispatch(ProviderKind::Vercel, &WorkerId::new("worker-1")?),
            &body,
        )
        .await?;
    let fake = &running.fake;
    until(|| fake.creates() == 0).await?;
    running.stop().await
}
