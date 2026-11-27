use std::error::Error;
use std::time::SystemTime;

use trogon_sandbox_contract::{LeaseId, TenantId};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn marker(id: &str) -> Result<PendingPurge, Box<dyn Error>> {
    Ok(PendingPurge {
        quota_reservation: None,
        lease_id: LeaseId::new(id),
        tenant: TenantId::new("acme")?,
        name: Some("build".into()),
        tag_revision: Some(1),
        release_announced: None,
        pending_refund: None,
        drive_claims: None,
        announcement: None,
        recorded_at: SystemTime::now(),
    })
}

#[test]
fn a_marker_without_a_pending_refund_still_reads() -> TestResult {
    let mut legacy = serde_json::to_value(marker("lease-a")?)?;
    legacy
        .as_object_mut()
        .ok_or("marker is not an object")?
        .remove("pending_refund");
    let read: PendingPurge = serde_json::from_value(legacy)?;
    assert_eq!(read.pending_refund, None);

    let mut owed = marker("lease-b")?;
    owed.pending_refund = Some(PendingRefund {
        cpu_seconds: 7,
        wall_seconds: 9,
    });
    let round_trip: PendingPurge = serde_json::from_slice(&serde_json::to_vec(&owed)?)?;
    assert_eq!(round_trip, owed);
    Ok(())
}

#[tokio::test]
async fn in_memory_store_round_trips_lists_and_clears() -> TestResult {
    let store = InMemoryPendingPurgeStore::new();
    let a = marker("lease-a")?;
    let b = marker("lease-b")?;
    store.put(&a).await?;
    store.put(&b).await?;

    assert_eq!(store.get(&a.lease_id).await?, Some(a.clone()));
    assert_eq!(store.all().await?.len(), 2);

    store.clear(&a.lease_id).await?;
    assert_eq!(store.get(&a.lease_id).await?, None);
    assert_eq!(store.all().await?, vec![b]);
    Ok(())
}
