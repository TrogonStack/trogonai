use std::sync::Arc;

use crate::quota::{QuotaStore, SnapshotQuota};
use trogon_sandbox_contract::{SnapshotDeleted, SnapshotPublished};

use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, OciArtifactRef, TenantId};

use super::*;
use crate::quota::InMemoryQuotaStore;
use crate::snapshot_accounting::accepted::Accepted;

fn sample_digest() -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )?)
}

async fn sample_event(quota: &dyn QuotaStore, bytes: u64) -> Result<SnapshotPublished, Box<dyn std::error::Error>> {
    let mut event = SnapshotPublished {
        lease_id: LeaseId::new("lease-1"),
        digest: sample_digest()?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
        tenant: TenantId::new("tenant-a")?,
        bytes,
        generation: None,
        correlation_id: None,
        tag: None,
    };
    register(quota, &mut event).await?;
    Ok(event)
}
async fn register(quota: &dyn QuotaStore, event: &mut SnapshotPublished) -> Result<(), Box<dyn std::error::Error>> {
    let store = quota.snapshot_store().ok_or("snapshot store")?;
    let reservation = store
        .reserve(
            &event.tenant,
            &event.lease_id,
            &format!("{}:{}:{}", event.lease_id.as_str(), event.digest, event.bytes),
        )
        .await
        .accepted()?;
    store
        .bind(
            &reservation,
            OciArtifactRef {
                registry: "registry.example.com".into(),
                repository: trogon_sandbox_protocol::snapshot_repository_for_tenant(&event.tenant),
                digest: event.digest.clone(),
                artifact_type: event.artifact_type,
            },
            &trogon_sandbox_contract::SnapshotReceipt {
                digest: event.digest.clone(),
                artifact_type: event.artifact_type,
                bytes: event.bytes,
            },
        )
        .await
        .accepted()?;
    event.generation = Some(reservation.generation);
    store.stage_publication(event).await.accepted()?;
    Ok(())
}
async fn complete_delete(quota: &dyn QuotaStore, event: &SnapshotPublished) -> Result<(), Box<dyn std::error::Error>> {
    let store = quota.snapshot_store().ok_or("snapshot store")?;
    let claim = store
        .claim_delete(&event.tenant, &event.digest)
        .await
        .accepted()?
        .ok_or("no claim")?;
    store.complete_delete(&claim).await.accepted()?;
    Ok(())
}
#[tokio::test]
async fn repeated_snapshot_publications_charge_the_same_artifact_once() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let defaults = SnapshotQuota::new(10, 1_000_000);
    let event = sample_event(&quota, 500).await?;
    for _ in 0..3 {
        handle_snapshot_published(live.as_ref(), &quota, defaults.clone(), event.clone()).await;
    }
    let record = quota.quota_record(&event.tenant).await?.ok_or("not found")?;
    assert_eq!((record.snapshot.count_used, record.snapshot.bytes_used), (1, 500));
    assert_eq!(live.get(&event.digest).await?.ok_or("not found")?.bytes, 500);
    Ok(())
}
#[tokio::test]
async fn repeated_snapshot_deletions_preserve_another_artifacts_budget() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let defaults = SnapshotQuota::new(10, 1_000_000);
    let first = sample_event(&quota, 500).await?;
    let mut other = sample_event(&quota, 300).await?;
    other.lease_id = LeaseId::new("lease-2");
    other.digest = Digest::new(format!("sha256:{}", "b".repeat(64)))?;
    register(&quota, &mut other).await?;
    for event in [first.clone(), other.clone()] {
        handle_snapshot_published(live.as_ref(), &quota, defaults.clone(), event).await;
    }
    complete_delete(&quota, &first).await?;
    for _ in 0..3 {
        handle_snapshot_deleted(
            live.as_ref(),
            &quota,
            SnapshotDeleted {
                digest: first.digest.clone(),
                tenant: first.tenant.clone(),
                bytes: first.bytes,
                generation: first.generation.clone(),
            },
        )
        .await;
    }
    let record = quota.quota_record(&first.tenant).await?.ok_or("not found")?;
    assert_eq!((record.snapshot.count_used, record.snapshot.bytes_used), (1, 300));
    assert!(live.get(&other.digest).await?.is_some());
    Ok(())
}
#[tokio::test]
async fn published_event_records_live_set_and_quota() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let defaults = SnapshotQuota::new(10, 1_000_000);

    handle_snapshot_published(live.as_ref(), &quota, defaults, sample_event(&quota, 500).await?).await;

    let entry = live.get(&sample_digest()?).await?.ok_or("not found")?;
    assert_eq!(entry.bytes, 500);
    assert_eq!(quota.active_lease_count(&TenantId::new("tenant-a")?).await?, 0);
    assert!(
        quota
            .record_snapshot_published(&TenantId::new("tenant-a")?, 100, SnapshotQuota::new(10, 1_000_000),)
            .await
            .is_ok()
    );
    Ok(())
}
#[tokio::test]
async fn publish_over_quota_fails_loudly_without_live_entry() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let defaults = SnapshotQuota::new(1, 1000);

    handle_snapshot_published(
        live.as_ref(),
        &quota,
        defaults.clone(),
        sample_event(&quota, 500).await?,
    )
    .await;

    handle_snapshot_published(live.as_ref(), &quota, defaults, sample_event(&quota, 100).await?).await;

    assert!(live.get(&sample_digest()?).await?.is_some());
    Ok(())
}
#[tokio::test]
async fn deleted_event_removes_live_set_and_quota() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    let defaults = SnapshotQuota::new(10, 1_000_000);

    let event = sample_event(&quota, 500).await?;
    handle_snapshot_published(live.as_ref(), &quota, defaults, event.clone()).await;

    complete_delete(&quota, &event).await?;
    handle_snapshot_deleted(
        live.as_ref(),
        &quota,
        SnapshotDeleted {
            digest: event.digest,
            tenant: event.tenant,
            bytes: event.bytes,
            generation: event.generation.clone(),
        },
    )
    .await;

    assert!(live.get(&sample_digest()?).await?.is_none());
    Ok(())
}
#[tokio::test]
async fn removing_another_tenants_snapshot_reads_as_not_found_and_keeps_it() -> Result<(), Box<dyn std::error::Error>> {
    let live = InMemoryLiveSnapshotStore::new();
    let quota = InMemoryQuotaStore::new();
    handle_snapshot_published(
        live.as_ref(),
        &quota,
        SnapshotQuota::new(10, 1_000_000),
        sample_event(&quota, 500).await?,
    )
    .await;
    let live_store: Arc<dyn LiveSnapshotStore> = live.clone();
    let remover = RegistrySnapshotRemover::new(
        live_store,
        Arc::new(trogon_sandbox_oci::OciClient::new(
            trogon_sandbox_oci::OciClientConfig::default(),
        )),
        "registry.example.com".to_string(),
    );

    let removal = remover.remove(&TenantId::new("tenant-b")?, &sample_digest()?).await?;

    assert_eq!(removal, SnapshotRemoval::NotFound);
    let entry = live.get(&sample_digest()?).await?.ok_or("not found")?;
    assert_eq!(entry.tenant, TenantId::new("tenant-a")?);
    Ok(())
}
