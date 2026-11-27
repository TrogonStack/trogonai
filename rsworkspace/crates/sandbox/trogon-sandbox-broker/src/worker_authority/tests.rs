use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseEpoch, LeaseId};

use super::*;
use crate::active_lease::tests::sample_record;
use crate::active_lease::{ActiveLeaseStoreError, InMemoryActiveLeaseStore};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn worker(id: &str) -> Result<WorkerId, Box<dyn std::error::Error>> {
    Ok(WorkerId::new(id)?)
}

fn request(worker_id: &str, leases: &[&str]) -> Result<WorkerAuthorityRequest, Box<dyn std::error::Error>> {
    Ok(WorkerAuthorityRequest {
        worker_id: worker(worker_id)?,
        lease_ids: leases.iter().map(|id| LeaseId::new(*id)).collect(),
    })
}

async fn put(
    store: &InMemoryActiveLeaseStore,
    id: &str,
    assigned: Option<&str>,
    status: LeaseStatus,
    epoch: Option<u64>,
) -> TestResult {
    let mut record = sample_record(id, "t", SystemTime::now())?;
    record.worker_id = assigned.map(worker).transpose()?;
    record.status = status;
    record.epoch = epoch.map(LeaseEpoch::new).transpose()?;
    store.put(&record).await?;
    Ok(())
}

#[tokio::test]
async fn it_returns_only_the_listed_leases_assigned_to_the_asker_with_their_epochs() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    put(&store, "mine", Some("w1"), LeaseStatus::Running, Some(3)).await?;
    put(&store, "moved", Some("w2"), LeaseStatus::Running, Some(4)).await?;
    put(&store, "legacy", None, LeaseStatus::Running, None).await?;
    put(&store, "done", Some("w1"), LeaseStatus::Released, Some(1)).await?;
    put(&store, "unlisted", Some("w1"), LeaseStatus::Running, Some(1)).await?;

    let response = answer(
        &store,
        &worker("w1")?,
        &request("w1", &["mine", "moved", "legacy", "done", "missing"])?,
    )
    .await;

    assert_eq!(
        response,
        Ok(WorkerAuthorityResponse {
            owned: vec![OwnedLease {
                lease_id: LeaseId::new("mine"),
                epoch: Some(LeaseEpoch::new(3)?),
            }],
            not_assigned: vec![LeaseId::new("moved"), LeaseId::new("done")],
        }),
        "an unknown lease is in neither list"
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_legacy_owner_does_not_grant_authority_to_any_worker() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    for status in [
        LeaseStatus::Pending,
        LeaseStatus::Running,
        LeaseStatus::Paused,
        LeaseStatus::Suspending,
        LeaseStatus::Suspended,
        LeaseStatus::Waking,
        LeaseStatus::Cancelling,
    ] {
        put(&store, "legacy", None, status, None).await?;
        for asker in ["w1", "w2"] {
            let response = answer(&store, &worker(asker)?, &request(asker, &["legacy"])?)
                .await
                .map_err(|refusal| format!("{refusal:?}"))?;
            assert!(
                response.owned.is_empty(),
                "a lease with no recorded owner is not positive authority for any asker"
            );
            assert!(
                response.not_assigned.is_empty(),
                "unknown ownership must not invent an assignment to another worker"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_body_naming_another_worker_than_the_subject_is_refused() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    put(&store, "l1", Some("w2"), LeaseStatus::Running, Some(1)).await?;
    assert_eq!(
        answer(&store, &worker("w1")?, &request("w2", &["l1"])?).await,
        Err(AuthorityRefusal::WorkerMismatch)
    );
    Ok(())
}

#[tokio::test]
async fn an_oversized_request_is_refused() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let ids: Vec<String> = (0..=MAX_LEASES_PER_REQUEST).map(|n| n.to_string()).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    assert_eq!(
        answer(&store, &worker("w1")?, &request("w1", &refs)?).await,
        Err(AuthorityRefusal::TooManyLeases)
    );
    Ok(())
}

struct BrokenStore;

#[async_trait]
impl ActiveLeaseStore for BrokenStore {
    async fn put(&self, _: &LeaseRecord) -> Result<(), ActiveLeaseStoreError> {
        Err(ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable")))
    }
    async fn get(&self, _: &LeaseId) -> Result<Option<LeaseRecord>, ActiveLeaseStoreError> {
        Err(ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable")))
    }
    async fn delete(&self, _: &LeaseId) -> Result<bool, ActiveLeaseStoreError> {
        Err(ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable")))
    }
    async fn all(&self) -> Result<Vec<LeaseRecord>, ActiveLeaseStoreError> {
        Err(ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable")))
    }
    async fn touch(&self, _: &LeaseId, _: SystemTime) -> Result<bool, ActiveLeaseStoreError> {
        Err(ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable")))
    }
    async fn replace_if_unchanged(&self, _: &LeaseRecord, _: &LeaseRecord) -> Result<bool, ActiveLeaseStoreError> {
        Err(ActiveLeaseStoreError::nats(std::io::Error::other("kv unavailable")))
    }
}

#[tokio::test]
async fn a_store_failure_is_not_answered_as_owning_nothing() -> TestResult {
    assert_eq!(
        answer(&BrokenStore, &worker("w1")?, &request("w1", &["l1"])?).await,
        Err(AuthorityRefusal::Store("kv unavailable".into()))
    );
    Ok(())
}
