use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseRecord, LeaseStatus, ProviderKind, WorkerId};

use std::error::Error;

use super::*;
use crate::active_lease::tests::sample_record;
use crate::active_lease::{ActiveLeaseStore, InMemoryActiveLeaseStore};

type TestResult = Result<(), Box<dyn Error>>;

fn worker(name: &str) -> Result<WorkerId, Box<dyn Error>> {
    Ok(WorkerId::new(name)?)
}

fn fence(name: &str, state: WorkerPlacementState) -> Result<WorkerFence, Box<dyn Error>> {
    Ok(WorkerFence {
        provider: ProviderKind::LocalDocker,
        worker_id: worker(name)?,
        state,
        reason: None,
        updated_at: SystemTime::now(),
    })
}

#[test]
fn keys_do_not_collide_for_distinct_worker_ids() -> TestResult {
    let a = fence_kv_key(ProviderKind::LocalDocker, &worker("a-b")?);
    let b = fence_kv_key(ProviderKind::LocalDocker, &worker("a_b")?);
    assert_ne!(a, b);
    assert!(!a.contains('/') && !b.contains('/'));
    Ok(())
}

#[tokio::test]
async fn view_follows_the_store_and_active_clears_the_fence() -> TestResult {
    let store = InMemoryWorkerFenceStore::new();
    let view = FenceView::new();
    let (w1, w2, w3) = (worker("w1")?, worker("w2")?, worker("w3")?);
    assert!(view.admits_placements(ProviderKind::LocalDocker, &w1));

    store.set(&fence("w1", WorkerPlacementState::Cordoned)?).await?;
    store.set(&fence("w2", WorkerPlacementState::Draining)?).await?;
    view.refresh(&store).await;
    assert!(!view.admits_placements(ProviderKind::LocalDocker, &w1));
    assert!(!view.admits_placements(ProviderKind::LocalDocker, &w2));
    assert!(view.admits_placements(ProviderKind::LocalDocker, &w3));
    assert!(view.admits_placements(ProviderKind::Daytona, &w1));

    store.set(&fence("w1", WorkerPlacementState::Active)?).await?;
    view.refresh(&store).await;
    assert!(view.admits_placements(ProviderKind::LocalDocker, &w1));
    assert_eq!(store.list().await?.len(), 1);
    Ok(())
}

struct FailingStore;

#[async_trait]
impl WorkerFenceStore for FailingStore {
    async fn get(&self, _: ProviderKind, _: &WorkerId) -> anyhow::Result<Option<WorkerFence>> {
        anyhow::bail!("down")
    }
    async fn set(&self, _: &WorkerFence) -> anyhow::Result<()> {
        anyhow::bail!("down")
    }
    async fn list(&self) -> anyhow::Result<Vec<WorkerFence>> {
        anyhow::bail!("down")
    }
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_cordon() -> TestResult {
    let view = FenceView::new();
    view.replace(&[fence("w1", WorkerPlacementState::Cordoned)?]);
    view.refresh(&FailingStore).await;
    assert!(!view.admits_placements(ProviderKind::LocalDocker, &worker("w1")?));
    Ok(())
}

fn leased(id: &str, worker_name: Option<&str>, status: LeaseStatus) -> Result<LeaseRecord, Box<dyn Error>> {
    let mut record = sample_record(id, "acme", SystemTime::now() + Duration::from_secs(60))?;
    record.provider = ProviderKind::LocalDocker;
    record.status = status;
    record.worker_id = worker_name.map(WorkerId::new).transpose()?;
    Ok(record)
}

#[tokio::test]
async fn drain_report_lists_only_live_leases_of_that_worker() -> TestResult {
    let active = InMemoryActiveLeaseStore::new();
    for record in [
        leased("b", Some("w1"), LeaseStatus::Running)?,
        leased("a", Some("w1"), LeaseStatus::Paused)?,
        leased("gone", Some("w1"), LeaseStatus::Released)?,
        leased("other", Some("w2"), LeaseStatus::Running)?,
        leased("anon", None, LeaseStatus::Running)?,
    ] {
        active.put(&record).await?;
    }
    let fences = InMemoryWorkerFenceStore::new();
    fences.set(&fence("w1", WorkerPlacementState::Draining)?).await?;

    let report = drain_report(&fences, &active, ProviderKind::LocalDocker, &worker("w1")?).await?;
    assert_eq!(report.state, WorkerPlacementState::Draining);
    let ids: Vec<_> = report.remaining.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
    assert!(!report.is_drained());

    let empty = drain_report(&fences, &active, ProviderKind::LocalDocker, &worker("w3")?).await?;
    assert!(empty.is_drained());
    assert_eq!(empty.state, WorkerPlacementState::Active);
    Ok(())
}
