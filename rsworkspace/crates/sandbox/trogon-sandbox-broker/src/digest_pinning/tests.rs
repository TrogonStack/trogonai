use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{Digest, LeaseFailed, LeaseFailedReason, LeaseId, SnapshotRestoreResult, TenantId};

use super::digest_pin_watcher::lease_id_from_snapshot_result_subject;

use std::error::Error;
use std::sync::{Mutex, PoisonError};

use super::*;
use crate::bus::BrokerBus;

type TestResult = Result<(), Box<dyn Error>>;

struct CaptureBus {
    published: Arc<Mutex<Vec<(String, Bytes)>>>,
}

#[async_trait]
impl BrokerBus for CaptureBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((subject.to_string(), payload));
        Ok(())
    }
}

fn digest(suffix: &str) -> Result<Digest, Box<dyn Error>> {
    let hex: String = suffix.chars().cycle().take(64).collect();
    Ok(Digest::new(format!("sha256:{hex}"))?)
}

type Published = Arc<Mutex<Vec<(String, Bytes)>>>;

fn capture() -> (Published, CaptureBus) {
    let published = Arc::new(Mutex::new(Vec::new()));
    let bus = CaptureBus {
        published: Arc::clone(&published),
    };
    (published, bus)
}

#[tokio::test]
async fn matching_digest_does_not_emit_lease_failed() -> TestResult {
    let pins = InMemorySnapshotPinStore::new();
    let (published, bus) = capture();

    let expected = digest("aa")?;
    let lease_id = LeaseId::new("lease-1");
    pins.pin(&lease_id, &expected).await?;

    handle_snapshot_restore_result(
        pins.as_ref(),
        &bus,
        None,
        lease_id,
        TenantId::new("t")?,
        SnapshotRestoreResult { digest: expected },
    )
    .await?;

    assert!(published.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    Ok(())
}

#[tokio::test]
async fn mismatched_digest_emits_lease_failed() -> TestResult {
    let pins = InMemorySnapshotPinStore::new();
    let (published, bus) = capture();

    let lease_id = LeaseId::new("lease-2");
    pins.pin(&lease_id, &digest("aa")?).await?;

    handle_snapshot_restore_result(
        pins.as_ref(),
        &bus,
        None,
        lease_id.clone(),
        TenantId::new("t")?,
        SnapshotRestoreResult { digest: digest("bb")? },
    )
    .await?;

    let messages = published.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(messages.len(), 1);
    let (subject, payload) = messages.first().ok_or("no message published")?;
    assert!(subject.contains("sandbox.lease.failed.lease-2"));
    let failed: LeaseFailed = serde_json::from_slice(payload)?;
    assert!(matches!(failed.reason, LeaseFailedReason::DigestMismatch(_)));
    Ok(())
}

#[test]
fn parses_lease_id_from_subject() -> TestResult {
    let id = lease_id_from_snapshot_result_subject("sandbox.evidence.01JABC.snapshot-result")
        .ok_or("lease id not parsed")?;
    assert_eq!(id.as_str(), "01JABC");
    Ok(())
}
