use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_broker::snapshot_tag::{
    InMemorySnapshotTagAuditor, InMemorySnapshotTagStore, SnapshotTagStore, SnapshotTagStoreError,
};
use trogon_sandbox_contract::{
    ArtifactType, Digest, EvictedArtifactAction, LeaseId, LeaseReleased, LeaseReleasedReason, RetentionPolicy,
    RetentionTagLimit, ServiceAccountId, SnapshotReceipt, SnapshotTag, SnapshotTagName, SnapshotTagRecord, TenantId,
};

use super::{auto_tag_snapshot, auto_tag_with_retry};
use crate::constants::MAX_TAG_CAS_ATTEMPTS;
use crate::release_tag::ReleaseTag;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest_of(hex: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(format!("sha256:{}", hex.to_string().repeat(64)).parse()?)
}

fn receipt_with(hex: char) -> Result<SnapshotReceipt, Box<dyn std::error::Error>> {
    Ok(SnapshotReceipt {
        digest: digest_of(hex)?,
        bytes: 1024,
        artifact_type: ArtifactType::SandboxRootfsV1,
    })
}

fn sample_event() -> Result<LeaseReleased, Box<dyn std::error::Error>> {
    Ok(LeaseReleased {
        lease_id: LeaseId::new("lease-auto-tag-1"),
        tenant: TenantId::new("tenant-auto-tag")?,
        reason: LeaseReleasedReason::Completed,
        snapshot_out: None,
        worker_id: None,
        correlation_id: None,
        restored_from: None,
        released_at: None,
    })
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn latest_tag(event: &LeaseReleased) -> Result<SnapshotTag, Box<dyn std::error::Error>> {
    Ok(SnapshotTag {
        tenant: event.tenant.clone(),
        name: SnapshotTagName::new("latest")?,
    })
}

fn store_with_audit() -> (InMemorySnapshotTagStore, Arc<InMemorySnapshotTagAuditor>) {
    let audit = Arc::new(InMemorySnapshotTagAuditor::new());
    (InMemorySnapshotTagStore::new(audit.clone()), audit)
}

async fn tag_named(
    store: &dyn SnapshotTagStore,
    event: &LeaseReleased,
    receipt: &SnapshotReceipt,
    tag_name: &str,
    retention: Option<RetentionPolicy>,
    observed_at: SystemTime,
) -> Result<(), SnapshotTagStoreError> {
    let release = ReleaseTag {
        event,
        receipt,
        tag_name,
        retention,
    };
    auto_tag_snapshot(store, &release, observed_at).await
}

async fn tag_at(
    store: &dyn SnapshotTagStore,
    receipt: &SnapshotReceipt,
    observed_at: SystemTime,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(tag_named(store, &sample_event()?, receipt, "latest", None, observed_at).await?)
}

async fn current_digest(store: &dyn SnapshotTagStore) -> Result<Digest, Box<dyn std::error::Error>> {
    let tag = latest_tag(&sample_event()?)?;
    Ok(store.get(&tag).await?.ok_or("the tag is not set")?.digest)
}

#[tokio::test]
async fn older_release_cannot_overwrite_newer_tag() -> TestResult {
    let (store, _) = store_with_audit();
    let newer = receipt_with('c')?;
    let older = receipt_with('d')?;
    tag_at(&store, &newer, at(2_000)).await?;
    tag_at(&store, &older, at(1_500)).await?;
    assert_eq!(current_digest(&store).await?, newer.digest);
    Ok(())
}

#[tokio::test]
async fn broker_release_time_orders_tags_instead_of_delivery_time() -> TestResult {
    let (store, _) = store_with_audit();
    let newer = receipt_with('c')?;
    let older = receipt_with('d')?;
    for (receipt, released_at, delivered_at) in [(&newer, at(2_000), at(9_000)), (&older, at(1_000), at(10_000))] {
        let mut event = sample_event()?;
        event.released_at = Some(released_at);
        tag_named(&store, &event, receipt, "latest", None, delivered_at).await?;
    }
    assert_eq!(current_digest(&store).await?, newer.digest);
    let tag = latest_tag(&sample_event()?)?;
    assert_eq!(store.get(&tag).await?.ok_or("the tag is not set")?.set_at, at(2_000));
    Ok(())
}

#[tokio::test]
async fn newer_or_equal_release_moves_tag() -> TestResult {
    let (store, _) = store_with_audit();
    let first = receipt_with('c')?;
    let equal = receipt_with('d')?;
    let newer = receipt_with('e')?;
    tag_at(&store, &first, at(2_000)).await?;
    tag_at(&store, &equal, at(2_000)).await?;
    assert_eq!(current_digest(&store).await?, equal.digest);
    tag_at(&store, &newer, at(3_000)).await?;
    assert_eq!(current_digest(&store).await?, newer.digest);
    Ok(())
}

#[tokio::test]
async fn tag_retry_converges_after_transient_failures() -> TestResult {
    let store = flaky(MAX_TAG_CAS_ATTEMPTS, false);
    let event = sample_event()?;
    let receipt = receipt_with('a')?;
    let release = ReleaseTag {
        event: &event,
        receipt: &receipt,
        tag_name: "latest",
        retention: None,
    };
    let backoff = [Duration::from_millis(1), Duration::from_millis(1)];
    auto_tag_with_retry(&store, &release, at(2_000), &backoff).await?;
    assert_eq!(current_digest(&store).await?, receipt.digest);
    Ok(())
}

#[tokio::test]
async fn tag_retry_is_bounded() -> TestResult {
    let store = flaky(0, true);
    let event = sample_event()?;
    let receipt = receipt_with('a')?;
    let release = ReleaseTag {
        event: &event,
        receipt: &receipt,
        tag_name: "latest",
        retention: None,
    };
    let backoff = [Duration::from_millis(1); 2];
    let result = auto_tag_with_retry(&store, &release, at(2_000), &backoff).await;
    assert!(matches!(result, Err(SnapshotTagStoreError::Backend(_))));
    Ok(())
}

#[tokio::test]
async fn stores_a_record_for_a_valid_tag_name() -> TestResult {
    let (store, _audit) = store_with_audit();
    let event = sample_event()?;
    let receipt = receipt_with('a')?;
    let retention = Some(RetentionPolicy {
        max_tags: RetentionTagLimit::new(3)?,
        max_age_days: 7,
        delete_evicted: EvictedArtifactAction::Delete,
    });

    tag_named(&store, &event, &receipt, "latest", retention, SystemTime::now()).await?;

    let record = store.get(&latest_tag(&event)?).await?.ok_or("the tag is not set")?;
    assert_eq!(record.digest, receipt.digest);
    assert_eq!(record.artifact_type, receipt.artifact_type);
    assert_eq!(record.set_by, ServiceAccountId::new("trogon-sandbox-snapshot-builder"));
    assert_eq!(record.retention, retention);
    Ok(())
}

#[tokio::test]
async fn skips_an_invalid_tag_name_without_error() -> TestResult {
    let (store, audit) = store_with_audit();
    let receipt = receipt_with('a')?;

    tag_named(
        &store,
        &sample_event()?,
        &receipt,
        "not a valid tag!",
        None,
        SystemTime::now(),
    )
    .await?;

    assert!(audit.events().await.is_empty());
    Ok(())
}

struct FlakyTagStore {
    inner: InMemorySnapshotTagStore,
    conflicts: AtomicUsize,
    backend_failure: bool,
    competing_create: Mutex<Option<SnapshotTagRecord>>,
}

#[async_trait]
impl SnapshotTagStore for FlakyTagStore {
    async fn get(&self, tag: &SnapshotTag) -> Result<Option<SnapshotTagRecord>, SnapshotTagStoreError> {
        self.inner.get(tag).await
    }

    async fn get_entry(&self, tag: &SnapshotTag) -> Result<Option<(SnapshotTagRecord, u64)>, SnapshotTagStoreError> {
        self.inner.get_entry(tag).await
    }

    async fn set(
        &self,
        tag: &SnapshotTag,
        record: SnapshotTagRecord,
        expected_revision: Option<u64>,
    ) -> Result<u64, SnapshotTagStoreError> {
        let competing = self.competing_create.lock().await.take();
        if let Some(record) = competing {
            self.inner.set(tag, record, Some(0)).await?;
        }
        if self.backend_failure {
            return Err(SnapshotTagStoreError::Backend("down".into()));
        }
        let left = self.conflicts.load(Ordering::SeqCst);
        if left > 0 {
            self.conflicts.store(left - 1, Ordering::SeqCst);
            return Err(SnapshotTagStoreError::RevisionConflict);
        }
        self.inner.set(tag, record, expected_revision).await
    }

    async fn clear(&self, tag: &SnapshotTag, expected_revision: u64) -> Result<(), SnapshotTagStoreError> {
        self.inner.clear(tag, expected_revision).await
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<(SnapshotTag, SnapshotTagRecord)>, SnapshotTagStoreError> {
        self.inner.list(tenant).await
    }

    async fn tenants(&self) -> Result<Vec<TenantId>, SnapshotTagStoreError> {
        self.inner.tenants().await
    }
}

fn flaky(conflicts: usize, backend_failure: bool) -> FlakyTagStore {
    let (inner, _) = store_with_audit();
    FlakyTagStore {
        inner,
        conflicts: AtomicUsize::new(conflicts),
        backend_failure,
        competing_create: Mutex::new(None),
    }
}

fn other_digest_record() -> Result<SnapshotTagRecord, Box<dyn std::error::Error>> {
    Ok(SnapshotTagRecord {
        digest: digest_of('b')?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        set_by: ServiceAccountId::new("someone"),
        set_at: at(1_000),
        retention: None,
    })
}

#[tokio::test]
async fn absent_tag_race_preserves_a_newer_concurrent_release() -> TestResult {
    let store = flaky(0, false);
    let newer = receipt_with('e')?;
    *store.competing_create.lock().await = Some(SnapshotTagRecord {
        digest: newer.digest.clone(),
        artifact_type: newer.artifact_type,
        set_by: ServiceAccountId::new("concurrent-builder"),
        set_at: at(3_000),
        retention: None,
    });

    tag_at(&store, &receipt_with('d')?, at(2_000)).await?;

    assert_eq!(current_digest(&store).await?, newer.digest);
    Ok(())
}

#[tokio::test]
async fn failure_leaves_the_previous_tag_and_reports_the_error() -> TestResult {
    let store = flaky(0, true);
    let event = sample_event()?;
    let tag = latest_tag(&event)?;
    let old = other_digest_record()?;
    store.inner.set(&tag, old.clone(), None).await?;

    let result = tag_named(&store, &event, &receipt_with('a')?, "latest", None, SystemTime::now()).await;

    assert!(matches!(result, Err(SnapshotTagStoreError::Backend(_))));
    let current = store.get(&tag).await?.ok_or("the tag is not set")?;
    assert_eq!(current.digest, old.digest);
    Ok(())
}

#[tokio::test]
async fn retries_revision_conflicts_then_converges() -> TestResult {
    let store = flaky(MAX_TAG_CAS_ATTEMPTS - 1, false);
    let event = sample_event()?;
    let tag = latest_tag(&event)?;
    store.inner.set(&tag, other_digest_record()?, None).await?;
    let receipt = receipt_with('a')?;

    tag_named(&store, &event, &receipt, "latest", None, SystemTime::now()).await?;

    let current = store.get(&tag).await?.ok_or("the tag is not set")?;
    assert_eq!(current.digest, receipt.digest);
    Ok(())
}

#[tokio::test]
async fn gives_up_after_bounded_conflicts_and_is_idempotent_on_retry() -> TestResult {
    let store = flaky(MAX_TAG_CAS_ATTEMPTS, false);
    let mut event = sample_event()?;
    event.released_at = Some(at(2_000));
    let receipt = receipt_with('a')?;

    let first = tag_named(&store, &event, &receipt, "latest", None, SystemTime::now()).await;
    assert!(matches!(first, Err(SnapshotTagStoreError::RevisionConflict)));
    let tag = latest_tag(&event)?;
    assert!(store.get(&tag).await?.is_none());

    tag_named(&store, &event, &receipt, "latest", None, SystemTime::now()).await?;
    let revision = store.get_entry(&tag).await?.ok_or("the tag is not set")?.1;
    tag_named(&store, &event, &receipt, "latest", None, SystemTime::now()).await?;
    assert_eq!(store.get_entry(&tag).await?.ok_or("the tag is not set")?.1, revision);
    Ok(())
}
