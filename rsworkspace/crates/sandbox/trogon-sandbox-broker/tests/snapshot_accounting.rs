use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use async_trait::async_trait;
use chrono::{Datelike, Utc};
use tokio::sync::Notify;
use trogon_sandbox_broker::quota::{
    InMemoryQuotaStore, NatsKvQuotaStore, QuotaRecord, QuotaStore, SnapshotQuota, quota_kv_key_for_month,
};
use trogon_sandbox_broker::snapshot_catalog::ArtifactDeleteError;
use trogon_sandbox_broker::snapshot_tracker::{
    InMemoryLiveSnapshotStore, LiveSnapshotStore, NatsKvLiveSnapshotStore, SnapshotLiveError, handle_snapshot_deleted,
    handle_snapshot_published,
};
use trogon_sandbox_contract::{Digest, LeaseId, LiveSnapshotEntry, SnapshotDeleted, SnapshotPublished, TenantId};

#[path = "../src/test_support.rs"]
mod test_support;

const GENERATION_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const GENERATION_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

trait Accepted<T> {
    fn accepted(self) -> Result<T, Box<dyn std::error::Error>>;
}

impl<T> Accepted<T> for Result<T, trogon_sandbox_contract::SnapshotAccountingRefusal> {
    fn accepted(self) -> Result<T, Box<dyn std::error::Error>> {
        self.map_err(|refusal| format!("snapshot accounting refused: {refusal:?}").into())
    }
}

fn publication() -> Result<SnapshotPublished, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "lease_id": "snapshot-accounting-lease", "digest": format!("sha256:{}", "a".repeat(64)),
        "artifact_type": "sandbox_rootfs_v1", "tenant": "snapshot-accounting-tenant", "bytes": 500,
        "generation": GENERATION_A,
    }))
}

fn deletion(
    event: &SnapshotPublished,
    generation: &trogon_sandbox_contract::SnapshotGeneration,
    tenant: &TenantId,
    bytes: u64,
) -> Result<SnapshotDeleted, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "digest": event.digest, "tenant": tenant, "bytes": bytes, "generation": generation,
    }))
}

async fn registered(
    quota: &dyn QuotaStore,
    mut event: SnapshotPublished,
) -> Result<SnapshotPublished, Box<dyn std::error::Error>> {
    let store = quota.snapshot_store().ok_or("no snapshot store")?;
    let capture = store
        .reserve(
            &event.tenant,
            &event.lease_id,
            &format!(
                "{}:{}:{}",
                event.lease_id.as_str(),
                event.digest,
                event.generation.as_ref().ok_or("missing generation")?
            ),
        )
        .await
        .accepted()?;
    let reference = trogon_sandbox_contract::OciArtifactRef {
        registry: "registry.example.test".into(),
        repository: trogon_sandbox_protocol::snapshot_repository_for_tenant(&event.tenant),
        digest: event.digest.clone(),
        artifact_type: event.artifact_type,
    };
    store
        .bind(
            &capture,
            reference,
            &trogon_sandbox_contract::SnapshotReceipt {
                digest: event.digest.clone(),
                artifact_type: event.artifact_type,
                bytes: event.bytes,
            },
        )
        .await
        .accepted()?;
    event.generation = Some(capture.generation);
    store.stage_publication(&event).await.accepted()?;
    Ok(event)
}
struct SuccessfulDelete;
#[async_trait]
impl trogon_sandbox_broker::snapshot_catalog::ArtifactDeleter for SuccessfulDelete {
    async fn delete(&self, _: &trogon_sandbox_contract::OciArtifactRef) -> Result<(), ArtifactDeleteError> {
        Ok(())
    }
}

async fn host_deleted(
    quota: &dyn QuotaStore,
    live: &dyn LiveSnapshotStore,
    event: &SnapshotPublished,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = quota.snapshot_store().ok_or("no snapshot store")?;
    let claim = trogon_sandbox_broker::snapshot_tracker::remove_accounted_snapshot(
        &store,
        live,
        &SuccessfulDelete,
        &event.tenant,
        &event.digest,
    )
    .await?
    .ok_or("no claim")?;
    assert_eq!(
        claim.generation,
        *event.generation.as_ref().ok_or("missing generation")?
    );
    Ok(())
}
async fn quota_handle(client: async_nats::Client) -> Result<Arc<NatsKvQuotaStore>, Box<dyn std::error::Error>> {
    Ok(Arc::new(
        NatsKvQuotaStore::new(jetstream::new(client), "snapshot-accounting-proof").await?,
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_live_store_survives_fresh_client_and_store_handles() -> Result<(), Box<dyn std::error::Error>> {
    let server = test_support::LocalNats::start().await;
    let event = publication()?;
    let entry = LiveSnapshotEntry {
        tenant: event.tenant,
        artifact_type: event.artifact_type,
        published_at: SystemTime::UNIX_EPOCH,
        bytes: event.bytes,
        generation: None,
    };
    let store = NatsKvLiveSnapshotStore::open(jetstream::new(server.client.clone())).await?;
    store.put(&event.digest, &entry).await?;
    server.client.flush().await?;
    let found = store.get(&event.digest).await?.ok_or("not found")?;
    assert_eq!(found, entry);
    drop(store);
    let reopened = NatsKvLiveSnapshotStore::open(jetstream::new(server.fresh_client().await)).await?;
    assert_eq!(reopened.get(&event.digest).await?, Some(entry));
    Ok(())
}
#[tokio::test]
async fn identical_content_retains_each_tenants_live_snapshot_ownership() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let event = publication()?;
    let first = LiveSnapshotEntry {
        tenant: event.tenant,
        artifact_type: event.artifact_type,
        published_at: SystemTime::UNIX_EPOCH,
        bytes: event.bytes,
        generation: None,
    };
    let second = LiveSnapshotEntry {
        tenant: TenantId::new("another-snapshot-tenant")?,
        ..first.clone()
    };
    live.put(&event.digest, &first).await?;
    live.put(&event.digest, &second).await?;
    assert_eq!(
        live.get_owned(&first.tenant, &event.digest).await?,
        Some(first),
        "a shared content digest must not replace another tenant's ownership"
    );
    assert_eq!(live.get_owned(&second.tenant, &event.digest).await?, Some(second));
    assert!(
        live.get(&event.digest).await.is_err(),
        "digest-only lookup must fail closed for multiple owners"
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_nats_quota_handles_charge_a_concurrent_artifact_once() -> Result<(), Box<dyn std::error::Error>> {
    let server = test_support::LocalNats::start().await;
    let first = quota_handle(server.client.clone()).await?;
    let second = quota_handle(server.fresh_client().await).await?;
    let live = InMemoryLiveSnapshotStore::new();
    let event = registered(first.as_ref(), publication()?).await?;
    let mut tasks = Vec::new();
    for index in 0..8 {
        let quota = if index % 2 == 0 { first.clone() } else { second.clone() };
        let live = live.clone();
        let event = event.clone();
        tasks.push(tokio::spawn(async move {
            handle_snapshot_published(live.as_ref(), quota.as_ref(), SnapshotQuota::new(100, 1_000_000), event).await;
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        for task in tasks {
            task.await?;
        }
        Ok::<(), tokio::task::JoinError>(())
    })
    .await??;
    let record = first.quota_record(&event.tenant).await?.ok_or("missing quota record")?;
    assert_eq!(
        (record.snapshot.count_used, record.snapshot.bytes_used),
        (1, 500),
        "independent brokers must atomically charge one artifact once"
    );
    Ok(())
}
struct BlockedProjection {
    entered: Notify,
    inner: Arc<InMemoryLiveSnapshotStore>,
}

#[async_trait]
impl LiveSnapshotStore for BlockedProjection {
    async fn put(&self, _digest: &Digest, _entry: &LiveSnapshotEntry) -> Result<(), SnapshotLiveError> {
        self.entered.notify_one();
        std::future::pending().await
    }

    async fn remove(&self, digest: &Digest) -> Result<(), SnapshotLiveError> {
        self.inner.remove(digest).await
    }

    async fn get(&self, digest: &Digest) -> Result<Option<LiveSnapshotEntry>, SnapshotLiveError> {
        self.inner.get(digest).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_after_charge_and_before_live_projection_repairs_without_recharging()
-> Result<(), Box<dyn std::error::Error>> {
    let server = test_support::LocalNats::start().await;
    let original_quota = quota_handle(server.client.clone()).await?;
    let blocked = Arc::new(BlockedProjection {
        entered: Notify::new(),
        inner: InMemoryLiveSnapshotStore::new(),
    });
    let event = registered(original_quota.as_ref(), publication()?).await?;
    let task_quota = original_quota.clone();
    let task_live = blocked.clone();
    let task_event = event.clone();
    let task = tokio::spawn(async move {
        handle_snapshot_published(
            task_live.as_ref(),
            task_quota.as_ref(),
            SnapshotQuota::new(100, 1_000_000),
            task_event,
        )
        .await;
    });
    tokio::time::timeout(Duration::from_secs(5), blocked.entered.notified()).await?;
    let before = original_quota
        .quota_record(&event.tenant)
        .await?
        .ok_or("missing quota record")?;
    assert_eq!((before.snapshot.count_used, before.snapshot.bytes_used), (1, 500));
    task.abort();
    assert!(task.await.err().is_some_and(|error| error.is_cancelled()));
    drop(original_quota);
    let recovered_quota = quota_handle(server.fresh_client().await).await?;
    let recovered_live = InMemoryLiveSnapshotStore::new();
    handle_snapshot_published(
        recovered_live.as_ref(),
        recovered_quota.as_ref(),
        SnapshotQuota::new(100, 1_000_000),
        event.clone(),
    )
    .await;
    let after = recovered_quota
        .quota_record(&event.tenant)
        .await?
        .ok_or("missing quota record")?;
    assert_eq!(
        (after.snapshot.count_used, after.snapshot.bytes_used),
        (1, 500),
        "a replay after broker loss must retain the original charge receipt"
    );
    assert_eq!(
        recovered_live.get(&event.digest).await?.ok_or("not found")?.bytes,
        500,
        "the fresh handler must repair the live projection"
    );
    Ok(())
}
#[tokio::test]
async fn forged_deletion_tenant_cannot_remove_or_refund_another_owner() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let first = registered(&quota, publication()?).await?;
    let mut other = first.clone();
    other.tenant = TenantId::new("another-snapshot-tenant")?;
    other.digest = Digest::new(format!("sha256:{}", "b".repeat(64)))?;
    let other = registered(&quota, other).await?;
    for event in [first.clone(), other.clone()] {
        handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), event).await;
    }
    handle_snapshot_deleted(
        live.as_ref(),
        &quota,
        deletion(
            &first,
            first.generation.as_ref().ok_or("missing generation")?,
            &other.tenant,
            first.bytes,
        )?,
    )
    .await;
    assert!(
        live.get_owned(&first.tenant, &first.digest).await?.is_some(),
        "a forged tenant must not remove the actual owner's artifact"
    );
    let other_budget = quota.quota_record(&other.tenant).await?.ok_or("missing quota record")?;
    assert_eq!(
        (other_budget.snapshot.count_used, other_budget.snapshot.bytes_used),
        (1, other.bytes)
    );
    Ok(())
}
#[tokio::test]
async fn deletion_uses_the_stored_charge_instead_of_forged_event_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let first = registered(&quota, publication()?).await?;
    let mut other = first.clone();
    other.lease_id = LeaseId::new("another-artifact");
    other.digest = Digest::new(format!("sha256:{}", "b".repeat(64)))?;
    other.bytes = 300;
    let other = registered(&quota, other).await?;
    for event in [first.clone(), other] {
        handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), event).await;
    }
    host_deleted(&quota, live.as_ref(), &first).await?;
    handle_snapshot_deleted(
        live.as_ref(),
        &quota,
        deletion(
            &first,
            first.generation.as_ref().ok_or("missing generation")?,
            &first.tenant,
            u64::MAX,
        )?,
    )
    .await;
    let budget = quota.quota_record(&first.tenant).await?.ok_or("missing quota record")?;
    assert_eq!(
        (budget.snapshot.count_used, budget.snapshot.bytes_used),
        (1, 300),
        "a forged refund size must not consume another artifact's budget"
    );
    Ok(())
}
#[tokio::test]
async fn deleted_generation_cannot_be_resurrected_by_publication_replay() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let event = registered(&quota, publication()?).await?;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), event.clone()).await;
    host_deleted(&quota, live.as_ref(), &event).await?;
    handle_snapshot_deleted(
        live.as_ref(),
        &quota,
        deletion(
            &event,
            event.generation.as_ref().ok_or("missing generation")?,
            &event.tenant,
            event.bytes,
        )?,
    )
    .await;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), event.clone()).await;
    assert!(
        live.get_owned(&event.tenant, &event.digest).await?.is_none(),
        "a deleted generation is terminal despite old publication delivery"
    );
    let budget = quota.quota_record(&event.tenant).await?.ok_or("missing quota record")?;
    assert_eq!((budget.snapshot.count_used, budget.snapshot.bytes_used), (0, 0));
    Ok(())
}
#[tokio::test]
async fn stale_deletion_cannot_remove_a_new_publication_generation() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let first = registered(&quota, publication()?).await?;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), first.clone()).await;
    host_deleted(&quota, live.as_ref(), &first).await?;
    let old_delete = deletion(
        &first,
        first.generation.as_ref().ok_or("missing generation")?,
        &first.tenant,
        first.bytes,
    )?;
    handle_snapshot_deleted(live.as_ref(), &quota, old_delete.clone()).await;
    let mut next = serde_json::to_value(&first)?;
    next["generation"] = GENERATION_B.into();
    next["lease_id"] = "a-new-capture".into();
    let next: SnapshotPublished = serde_json::from_value(next)?;
    let next = registered(&quota, next).await?;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), next.clone()).await;
    handle_snapshot_deleted(live.as_ref(), &quota, old_delete).await;
    assert!(
        live.get_owned(&next.tenant, &next.digest).await?.is_some(),
        "an exact old generation cannot delete a later incarnation"
    );
    let budget = quota.quota_record(&next.tenant).await?.ok_or("missing quota record")?;
    assert_eq!(
        (budget.snapshot.count_used, budget.snapshot.bytes_used),
        (1, next.bytes)
    );
    Ok(())
}
#[tokio::test]
async fn missing_generation_does_not_invent_snapshot_authority() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let mut legacy = serde_json::to_value(publication()?)?;
    legacy.as_object_mut().ok_or("not an object")?.remove("generation");
    let legacy: SnapshotPublished = serde_json::from_value(legacy)?;
    handle_snapshot_published(
        live.as_ref(),
        &quota,
        SnapshotQuota::new(100, 1_000_000),
        legacy.clone(),
    )
    .await;
    assert!(
        live.get_owned(&legacy.tenant, &legacy.digest).await?.is_none(),
        "legacy decoding alone does not prove publication generation"
    );
    Ok(())
}
#[test]
fn malformed_generation_is_rejected_at_the_public_wire() -> Result<(), Box<dyn std::error::Error>> {
    let mut malformed = serde_json::to_value(publication()?)?;
    malformed["generation"] = "not-a-host-generation".into();
    assert!(serde_json::from_value::<SnapshotPublished>(malformed).is_err());
    Ok(())
}
#[test]
fn snapshot_count_overflow_is_a_refusal_even_with_maximum_cap() {
    let mut quota = SnapshotQuota::new(u64::MAX, u64::MAX);
    quota.count_used = u64::MAX;
    assert!(
        quota.would_exceed_publish(1),
        "saturating arithmetic must not hide a count overflow"
    );
}

#[test]
fn snapshot_byte_overflow_is_a_refusal_even_with_maximum_cap() {
    let mut quota = SnapshotQuota::new(u64::MAX, u64::MAX);
    quota.bytes_used = u64::MAX;
    assert!(
        quota.would_exceed_publish(1),
        "saturating arithmetic must not hide a byte overflow"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiring_monthly_budget_does_not_erase_live_snapshot_usage() -> Result<(), Box<dyn std::error::Error>> {
    let server = test_support::LocalNats::start().await;
    let quota = quota_handle(server.client.clone()).await?;
    let live = InMemoryLiveSnapshotStore::new();
    let event = registered(quota.as_ref(), publication()?).await?;
    handle_snapshot_published(
        live.as_ref(),
        quota.as_ref(),
        SnapshotQuota::new(100, 1_000_000),
        event.clone(),
    )
    .await;
    let now = Utc::now();
    let key = quota_kv_key_for_month(&event.tenant, now.year(), now.month());
    let mut config = quota.kv.stream.cached_info().config.clone();
    quota
        .kv
        .put(&key, serde_json::to_vec(&QuotaRecord::new(3600, 7200))?.into())
        .await?;
    assert!(quota.kv.entry(&key).await?.is_some());
    config.max_age = Duration::from_millis(100);
    config.duplicate_window = Duration::from_millis(100);
    jetstream::new(server.client.clone()).update_stream(config).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while quota.kv.entry(&key).await?.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), async_nats::jetstream::kv::EntryError>(())
    })
    .await??;
    let state = quota.quota_record(&event.tenant).await?.ok_or("missing quota record")?;
    assert_eq!((state.snapshot.count_used, state.snapshot.bytes_used), (1, event.bytes));
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreceipted_legacy_snapshot_usage_is_quarantined_without_erasing_budget()
-> Result<(), Box<dyn std::error::Error>> {
    let server = test_support::LocalNats::start().await;
    let quota = quota_handle(server.client.clone()).await?;
    let event = registered(quota.as_ref(), publication()?).await?;
    let mut legacy = QuotaRecord::new(3600, 7200);
    legacy.snapshot = SnapshotQuota::new(100, 1_000_000);
    legacy.snapshot.count_used = 2;
    legacy.snapshot.bytes_used = 1_000;
    let now = Utc::now();
    let key = quota_kv_key_for_month(&event.tenant, now.year(), now.month());
    quota.kv.put(&key, serde_json::to_vec(&legacy)?.into()).await?;
    let live = InMemoryLiveSnapshotStore::new();
    handle_snapshot_published(
        live.as_ref(),
        quota.as_ref(),
        SnapshotQuota::new(100, 1_000_000),
        event.clone(),
    )
    .await;
    assert!(
        live.get_owned(&event.tenant, &event.digest).await?.is_none(),
        "unknown old obligations require reconciliation before new grants"
    );
    let retained: QuotaRecord = serde_json::from_slice(&quota.kv.get(&key).await?.ok_or("not found")?)?;
    assert_eq!((retained.snapshot.count_used, retained.snapshot.bytes_used), (2, 1_000));
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_snapshot_caps_survive_replay_with_different_caller_defaults() -> Result<(), Box<dyn std::error::Error>>
{
    let server = test_support::LocalNats::start().await;
    let quota = quota_handle(server.client.clone()).await?;
    let event = registered(quota.as_ref(), publication()?).await?;
    let mut configured = QuotaRecord::new(3600, 7200);
    configured.snapshot = SnapshotQuota::new(10, 50_000);
    let now = Utc::now();
    quota
        .kv
        .put(
            quota_kv_key_for_month(&event.tenant, now.year(), now.month()),
            serde_json::to_vec(&configured)?.into(),
        )
        .await?;
    let live = InMemoryLiveSnapshotStore::new();
    for defaults in [SnapshotQuota::new(100, 1_000_000), SnapshotQuota::new(1, 500)] {
        handle_snapshot_published(live.as_ref(), quota.as_ref(), defaults, event.clone()).await;
    }
    let state = quota.quota_record(&event.tenant).await?.ok_or("missing quota record")?;
    assert_eq!((state.snapshot.count_cap, state.snapshot.bytes_cap), (10, 50_000));
    assert_eq!((state.snapshot.count_used, state.snapshot.bytes_used), (1, event.bytes));
    Ok(())
}
#[tokio::test]
async fn an_unregistered_generation_is_not_publication_authority() -> Result<(), Box<dyn std::error::Error>> {
    let quota = InMemoryQuotaStore::new();
    let live = InMemoryLiveSnapshotStore::new();
    let response = trogon_sandbox_broker::snapshot_tracker::account_snapshot_published(
        live.as_ref(),
        &quota,
        SnapshotQuota::default(),
        &publication()?,
    )
    .await;
    assert!(matches!(
        response,
        trogon_sandbox_contract::SnapshotAccountingResponse::Refused {
            reason: trogon_sandbox_contract::SnapshotAccountingRefusal::UnknownGeneration,
        }
    ));
    assert!(quota.quota_record(&publication()?.tenant).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn deletion_fences_bound_captures_even_before_their_handlers_run() -> Result<(), Box<dyn std::error::Error>> {
    let quota = InMemoryQuotaStore::new();
    let live = InMemoryLiveSnapshotStore::new();
    let first = registered(&quota, publication()?).await?;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::default(), first.clone()).await;
    let mut delayed = publication()?;
    delayed.lease_id = LeaseId::new("delayed-capture");
    let delayed = registered(&quota, delayed).await?;
    host_deleted(&quota, live.as_ref(), &first).await?;
    let response = trogon_sandbox_broker::snapshot_tracker::account_snapshot_published(
        live.as_ref(),
        &quota,
        SnapshotQuota::default(),
        &delayed,
    )
    .await;
    assert!(matches!(
        response,
        trogon_sandbox_contract::SnapshotAccountingResponse::Committed {
            disposition: trogon_sandbox_contract::SnapshotAccountingDisposition::Deleted,
            ..
        }
    ));
    assert!(live.get_owned(&first.tenant, &first.digest).await?.is_none());
    let budget = quota.quota_record(&first.tenant).await?.ok_or("missing quota record")?;
    assert_eq!((budget.snapshot.count_used, budget.snapshot.bytes_used), (0, 0));
    Ok(())
}
#[tokio::test]
async fn unbound_capture_and_registry_deletion_claims_exclude_each_other() -> Result<(), Box<dyn std::error::Error>> {
    let quota = InMemoryQuotaStore::new();
    let live = InMemoryLiveSnapshotStore::new();
    let first = registered(&quota, publication()?).await?;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::default(), first.clone()).await;
    let store = quota.snapshot_store().ok_or("no snapshot store")?;
    let unbound = store
        .reserve(&first.tenant, &LeaseId::new("unbound"), "unbound")
        .await
        .accepted()?;
    assert!(matches!(
        store.claim_delete(&first.tenant, &first.digest).await,
        Err(trogon_sandbox_contract::SnapshotAccountingRefusal::Pending)
    ));
    store.abandon_unexported(&unbound).await.accepted()?;
    let claim = store
        .claim_delete(&first.tenant, &first.digest)
        .await
        .accepted()?
        .ok_or("no claim")?;
    assert!(matches!(
        store
            .reserve(&first.tenant, &first.lease_id, "concurrent-new-capture")
            .await,
        Err(trogon_sandbox_contract::SnapshotAccountingRefusal::Pending)
    ));
    assert!(matches!(
        store.claim_delete(&first.tenant, &first.digest).await,
        Err(trogon_sandbox_contract::SnapshotAccountingRefusal::Pending)
    ));
    store.complete_delete(&claim).await.accepted()?;
    assert!(
        store
            .reserve(&first.tenant, &first.lease_id, "after-delete")
            .await
            .is_ok()
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staged_publication_recovers_after_process_loss_before_queue_publish() -> Result<(), Box<dyn std::error::Error>>
{
    let server = test_support::LocalNats::start().await;
    let original = quota_handle(server.client.clone()).await?;
    let event = registered(original.as_ref(), publication()?).await?;
    drop(original);
    let reopened = quota_handle(server.fresh_client().await).await?;
    let store = reopened.snapshot_store().ok_or("no snapshot store")?;
    assert_eq!(store.recover_publications().await.accepted()?, 1);
    let mut stream = jetstream::new(server.client.clone())
        .get_stream("sandbox-snapshot-tag-recovery")
        .await?;
    assert_eq!(stream.info().await?.state.messages, 1);
    assert_eq!(store.recover_publications().await.accepted()?, 0);
    assert_eq!(stream.info().await?.state.messages, 1);
    let live = InMemoryLiveSnapshotStore::new();
    handle_snapshot_published(
        live.as_ref(),
        reopened.as_ref(),
        SnapshotQuota::default(),
        event.clone(),
    )
    .await;
    let budget = reopened
        .quota_record(&event.tenant)
        .await?
        .ok_or("missing quota record")?;
    assert_eq!(
        (budget.snapshot.count_used, budget.snapshot.bytes_used),
        (1, event.bytes)
    );
    Ok(())
}
struct HeldRegistryDelete {
    entered: Notify,
    finish: Notify,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl trogon_sandbox_broker::snapshot_catalog::ArtifactDeleter for HeldRegistryDelete {
    async fn delete(&self, _: &trogon_sandbox_contract::OciArtifactRef) -> Result<(), ArtifactDeleteError> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            self.finish.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn a_pending_deletion_cannot_grant_the_registry_effect_to_a_second_caller()
-> Result<(), Box<dyn std::error::Error>> {
    let quota = InMemoryQuotaStore::new();
    let live = InMemoryLiveSnapshotStore::new();
    let event = registered(&quota, publication()?).await?;
    handle_snapshot_published(live.as_ref(), &quota, SnapshotQuota::default(), event.clone()).await;
    let store = quota.snapshot_store().ok_or("no snapshot store")?;
    let registry = Arc::new(HeldRegistryDelete {
        entered: Notify::new(),
        finish: Notify::new(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let (task_store, task_live, task_registry, task_event) =
        (store.clone(), live.clone(), registry.clone(), event.clone());
    let first = tokio::spawn(async move {
        trogon_sandbox_broker::snapshot_tracker::remove_accounted_snapshot(
            &task_store,
            task_live.as_ref(),
            task_registry.as_ref(),
            &task_event.tenant,
            &task_event.digest,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), registry.entered.notified()).await?;
    let second = trogon_sandbox_broker::snapshot_tracker::remove_accounted_snapshot(
        &store,
        live.as_ref(),
        registry.as_ref(),
        &event.tenant,
        &event.digest,
    )
    .await;
    registry.finish.notify_one();
    first.await??.ok_or("no claim")?;
    assert!(
        second.is_err(),
        "a pending claim is not permission for a second physical registry deletion"
    );
    assert_eq!(registry.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}
#[tokio::test]
async fn accounting_cannot_restamp_a_generation_with_an_unstaged_tag_or_correlation()
-> Result<(), Box<dyn std::error::Error>> {
    let quota = InMemoryQuotaStore::new();
    let live = InMemoryLiveSnapshotStore::new();
    let staged = registered(&quota, publication()?).await?;
    let mut forged_tag = staged.clone();
    forged_tag.tag = Some(trogon_sandbox_contract::SnapshotPublishedTag {
        name: trogon_sandbox_contract::SnapshotTagName::new("unstaged-tag")?,
        set_at: SystemTime::now(),
        retention: None,
    });
    let mut forged_correlation = staged.clone();
    forged_correlation.correlation_id = Some(trogon_sandbox_contract::CorrelationId::new("unstaged-correlation"));
    for event in [forged_tag, forged_correlation] {
        let response = trogon_sandbox_broker::snapshot_tracker::account_snapshot_published(
            live.as_ref(),
            &quota,
            SnapshotQuota::default(),
            &event,
        )
        .await;
        assert!(
            matches!(
                response,
                trogon_sandbox_contract::SnapshotAccountingResponse::Refused {
                    reason: trogon_sandbox_contract::SnapshotAccountingRefusal::MetadataMismatch,
                }
            ),
            "only the host-staged complete publication intent can confirm accounting: {response:?}"
        );
    }
    assert!(quota.quota_record(&staged.tenant).await?.is_none());
    Ok(())
}
