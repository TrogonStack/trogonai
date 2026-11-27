use std::error::Error;
use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{LeaseId, LeaseReleased, LeaseReleasedReason, TenantId};

use crate::pending_purge::PendingRefund;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn marker(id: &str) -> Result<PendingRelease, Box<dyn Error>> {
    let tenant = TenantId::new("acme")?;
    Ok(PendingRelease {
        quota_reservation: None,
        lease_id: LeaseId::new(id),
        tenant: tenant.clone(),
        name: None,
        refund: PendingRefund {
            cpu_seconds: 3,
            wall_seconds: 4,
        },
        drive_claims: Vec::new(),
        announcement: LeaseReleased {
            lease_id: LeaseId::new(id),
            tenant,
            reason: LeaseReleasedReason::Cancelled,
            snapshot_out: None,
            worker_id: None,
            correlation_id: None,
            restored_from: None,
            released_at: None,
        },
        recorded_at: SystemTime::now(),
    })
}

#[tokio::test]
async fn create_keeps_the_first_marker_of_a_lease() -> TestResult {
    let store = InMemoryPendingReleaseStore::new();
    let first = marker("lease-a")?;
    let mut second = marker("lease-a")?;
    second.recorded_at += Duration::from_secs(3600);

    assert_eq!(store.create(&first).await?, (first.clone(), true));
    assert_eq!(store.create(&second).await?, (first.clone(), false));
    assert_eq!(store.all().await?, vec![first.clone()]);

    store.clear(&first.lease_id).await?;
    assert!(store.all().await?.is_empty());
    Ok(())
}
