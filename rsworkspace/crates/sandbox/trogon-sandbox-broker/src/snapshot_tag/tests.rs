use std::str::FromStr;

use std::time::SystemTime;

use super::tag_keys::legacy_tag_kv_key;
use crate::constants::SNAPSHOT_TAG_KV_HISTORY as TAG_KV_HISTORY;
use async_nats::jetstream::{self, kv};
use trogon_sandbox_contract::{Digest, ServiceAccountId, SnapshotTag, SnapshotTagName, SnapshotTagRecord, TenantId};
use trogon_sandbox_protocol::subjects::SNAPSHOT_TAG_KV_BUCKET;

use std::sync::Arc;

use trogon_sandbox_contract::ArtifactType;

use super::*;

fn test_tag(tenant: &str, name: &str) -> Result<SnapshotTag, Box<dyn std::error::Error>> {
    Ok(SnapshotTag {
        tenant: TenantId::new(tenant)?,
        name: SnapshotTagName::new(name)?,
    })
}
fn test_record(digest_hex: &str) -> Result<SnapshotTagRecord, Box<dyn std::error::Error>> {
    Ok(SnapshotTagRecord {
        digest: Digest::from_str(digest_hex)?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
        set_by: ServiceAccountId::new("test-operator"),
        set_at: SystemTime::now(),
        retention: None,
    })
}
fn in_memory_store() -> InMemorySnapshotTagStore {
    InMemorySnapshotTagStore::new(Arc::new(InMemorySnapshotTagAuditor::new()))
}

async fn assert_zero_revision_creates_only_an_absent_tag(
    store: &dyn SnapshotTagStore,
) -> Result<(), Box<dyn std::error::Error>> {
    let tag = test_tag("acme", "create-only")?;
    let (first, second) = tokio::join!(
        store.set(&tag, test_record(DIGEST_A)?, Some(0)),
        store.set(&tag, test_record(DIGEST_B)?, Some(0)),
    );
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "concurrent create-only writers must have one winner: {first:?}, {second:?}"
    );
    assert!(
        matches!(&first, Err(SnapshotTagStoreError::RevisionConflict))
            || matches!(&second, Err(SnapshotTagStoreError::RevisionConflict))
    );
    let expected = if first.is_ok() { DIGEST_A } else { DIGEST_B };
    let (saved, revision) = store.get_entry(&tag).await?.ok_or("not found")?;
    assert_eq!(saved.digest.as_str(), expected);
    assert!(revision > 0);
    assert!(matches!(
        store.set(&tag, test_record(DIGEST_C)?, Some(0)).await,
        Err(SnapshotTagStoreError::RevisionConflict)
    ));
    let (unchanged, same_revision) = store.get_entry(&tag).await?.ok_or("not found")?;
    assert_eq!(unchanged.digest.as_str(), expected);
    assert_eq!(same_revision, revision);
    Ok(())
}
#[tokio::test]
async fn zero_revision_creates_only_an_absent_tag_in_memory() -> Result<(), Box<dyn std::error::Error>> {
    assert_zero_revision_creates_only_an_absent_tag(&in_memory_store()).await?;
    Ok(())
}
#[tokio::test]
async fn zero_revision_creates_only_an_absent_tag_in_nats() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    assert_zero_revision_creates_only_an_absent_tag(&nats_store(&server).await?).await?;
    Ok(())
}
const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

async fn nats_store(
    server: &crate::test_support::LocalNats,
) -> Result<NatsKvSnapshotTagStore, Box<dyn std::error::Error>> {
    Ok(NatsKvSnapshotTagStore::new(
        jetstream::new(server.client.clone()),
        Arc::new(InMemorySnapshotTagAuditor::new()),
    )
    .await?)
}

async fn assert_folding_pairs_stay_independent(store: &dyn SnapshotTagStore) -> Result<(), Box<dyn std::error::Error>> {
    let a = test_tag("acme", "x_y")?;
    let b = test_tag("acme_x", "y")?;
    let c = test_tag("acme", "x.y")?;
    assert_eq!(legacy_tag_kv_key(&a), legacy_tag_kv_key(&b));
    assert_eq!(legacy_tag_kv_key(&a), legacy_tag_kv_key(&c));
    store.set(&a, test_record(DIGEST_A)?, None).await?;
    store.set(&b, test_record(DIGEST_B)?, None).await?;
    store.set(&c, test_record(DIGEST_C)?, None).await?;
    assert_eq!(store.get(&a).await?.ok_or("not found")?.digest.as_str(), DIGEST_A);
    assert_eq!(store.get(&b).await?.ok_or("not found")?.digest.as_str(), DIGEST_B);
    assert_eq!(store.get(&c).await?.ok_or("not found")?.digest.as_str(), DIGEST_C);
    let names = |tags: Vec<(SnapshotTag, SnapshotTagRecord)>| {
        tags.into_iter()
            .map(|(tag, _)| tag.name.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(store.list(&a.tenant).await?), ["x.y", "x_y"]);
    assert_eq!(names(store.list(&b.tenant).await?), ["y"]);
    Ok(())
}
#[tokio::test]
async fn in_memory_folding_pairs_stay_independent() -> Result<(), Box<dyn std::error::Error>> {
    assert_folding_pairs_stay_independent(&in_memory_store()).await?;
    Ok(())
}
#[tokio::test]
async fn nats_folding_pairs_stay_independent() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    assert_folding_pairs_stay_independent(&nats_store(&server).await?).await?;
    Ok(())
}
#[tokio::test]
async fn nats_legacy_record_is_read_then_moved_to_its_current_key() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = nats_store(&server).await?;
    let tag = test_tag("acme.x", "y")?;
    let legacy = legacy_tag_kv_key(&tag);
    assert_ne!(legacy, tag_kv_key(&tag));
    store
        .kv
        .put(legacy.as_str(), serde_json::to_vec(&test_record(DIGEST_A)?)?.into())
        .await?;

    let (record, revision) = store.get_entry(&tag).await?.ok_or("not found")?;
    assert_eq!(record.digest.as_str(), DIGEST_A);
    assert_eq!(store.list(&tag.tenant).await?.len(), 1);

    let moved = store.set(&tag, test_record(DIGEST_B)?, Some(revision)).await?;
    assert!(store.kv.get(legacy.as_str()).await?.is_none());
    let (record, current) = store.get_entry(&tag).await?.ok_or("not found")?;
    assert_eq!(record.digest.as_str(), DIGEST_B);
    assert_eq!(current, moved);
    let listed = store.list(&tag.tenant).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, tag);

    store.clear(&tag, moved).await?;
    assert!(store.get(&tag).await?.is_none());
    Ok(())
}
#[tokio::test]
async fn nats_stamped_record_under_a_shared_legacy_key_is_not_read_by_another_tag()
-> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = nats_store(&server).await?;
    let owner = test_tag("acme", "x_y")?;
    let other = test_tag("acme_x", "y")?;
    assert_eq!(tag_kv_key(&owner), legacy_tag_kv_key(&other));
    store.set(&owner, test_record(DIGEST_A)?, None).await?;

    assert!(store.get(&other).await?.is_none());
    assert!(store.list(&other.tenant).await?.is_empty());
    store.set(&other, test_record(DIGEST_B)?, None).await?;
    assert_eq!(store.get(&owner).await?.ok_or("not found")?.digest.as_str(), DIGEST_A);
    assert_eq!(store.get(&other).await?.ok_or("not found")?.digest.as_str(), DIGEST_B);
    Ok(())
}
#[tokio::test]
async fn in_memory_set_and_get_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let store = in_memory_store();
    let tag = test_tag("acme", "prod")?;
    let record = test_record("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
    let revision = store.set(&tag, record.clone(), None).await?;
    assert_eq!(revision, 1);
    let loaded = store.get(&tag).await?.ok_or("not found")?;
    assert_eq!(loaded.digest, record.digest);
    Ok(())
}
#[tokio::test]
async fn in_memory_clear_requires_revision() -> Result<(), Box<dyn std::error::Error>> {
    let store = in_memory_store();
    let tag = test_tag("acme", "prod")?;
    let record = test_record("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")?;
    let revision = store.set(&tag, record, None).await?;
    store.clear(&tag, revision).await?;
    assert!(store.get(&tag).await?.is_none());
    Ok(())
}
#[tokio::test]
async fn in_memory_revision_conflict_on_stale_set() -> Result<(), Box<dyn std::error::Error>> {
    let store = in_memory_store();
    let tag = test_tag("acme", "prod")?;
    let record_a = test_record("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")?;
    let record_b = test_record("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")?;
    let rev = store.set(&tag, record_a, None).await?;
    let err = store
        .set(&tag, record_b, Some(rev - 1))
        .await
        .err()
        .ok_or("stale revision")?;
    assert!(matches!(err, SnapshotTagStoreError::RevisionConflict));
    Ok(())
}
#[tokio::test]
async fn in_memory_concurrent_sets_are_linearizable() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(in_memory_store());
    let tag = test_tag("concurrent", "prod")?;

    let mut handles = Vec::new();
    for i in 0..4 {
        let store = Arc::clone(&store);
        let tag = tag.clone();
        let hex = format!("{i:064x}");
        let digest = format!("sha256:{hex}");
        let record = test_record(&digest)?;
        handles.push(tokio::spawn(async move { store.set(&tag, record, None).await }));
    }

    let mut revisions = Vec::new();
    for handle in handles {
        revisions.push(handle.await??);
    }
    revisions.sort_unstable();
    revisions.dedup();
    assert_eq!(
        revisions.len(),
        4,
        "each concurrent set must produce a distinct revision"
    );

    let final_record = store.get(&tag).await?.ok_or("not found")?;
    assert!(
        final_record.digest.as_str().starts_with("sha256:0000000000000000")
            || final_record.digest.as_str().starts_with("sha256:000000000001")
            || final_record.digest.as_str().starts_with("sha256:000000000002")
            || final_record.digest.as_str().starts_with("sha256:000000000003")
    );
    Ok(())
}
async fn make_nats_store(
    bucket_suffix: &str,
) -> Result<(NatsKvSnapshotTagStore, async_nats::jetstream::Context), Box<dyn std::error::Error>> {
    let client = async_nats::connect("nats://127.0.0.1:4222").await?;
    let js = async_nats::jetstream::new(client);
    let audit = Arc::new(JetStreamSnapshotTagAuditor::new(js.clone(), 1).await?);
    let kv = js
        .create_key_value(kv::Config {
            bucket: format!("{SNAPSHOT_TAG_KV_BUCKET}-{bucket_suffix}"),
            history: TAG_KV_HISTORY,
            ..Default::default()
        })
        .await?;
    let store = NatsKvSnapshotTagStore { kv, audit };
    Ok((store, js))
}

async fn cleanup_test_bucket(bucket: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = async_nats::connect("nats://127.0.0.1:4222").await?;
    let js = async_nats::jetstream::new(client);
    let _ = js.delete_key_value(bucket).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn nats_kv_concurrent_tag_sets_do_not_lose_updates() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{SNAPSHOT_TAG_KV_BUCKET}-{suffix}");
    let (store, js) = make_nats_store(&suffix).await?;
    let store = Arc::new(store);
    // The audit stream outlives the bucket, so a per-run tenant keeps
    // events from earlier runs out of the history count.
    let tenant = format!("nats-concurrent-{}", suffix.to_lowercase());
    let tag = test_tag(&tenant, "prod")?;

    let mut handles = Vec::new();
    for i in 0..4 {
        let store = Arc::clone(&store);
        let tag = tag.clone();
        let hex = format!("{i:064x}");
        let digest = format!("sha256:{hex}");
        let record = test_record(&digest)?;
        handles.push(tokio::spawn(async move { store.set(&tag, record, None).await }));
    }

    let mut revisions = Vec::new();
    for handle in handles {
        revisions.push(handle.await??);
    }
    revisions.sort_unstable();
    revisions.dedup();
    assert_eq!(revisions.len(), 4, "distinct revisions: {revisions:?}");

    let final_record = store.get(&tag).await?.ok_or("not found")?;
    assert!(final_record.digest.as_str().starts_with("sha256:"));

    let history = read_tag_audit_history(js, &TenantId::new(&tenant)?, &SnapshotTagName::new("prod")?).await?;
    assert_eq!(history.len(), 4, "one audit event per set");

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn nats_kv_clear_keeps_a_tag_set_after_the_expected_revision() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{SNAPSHOT_TAG_KV_BUCKET}-{suffix}");
    let (store, _js) = make_nats_store(&suffix).await?;
    let tag = test_tag("nats-clear", "prod")?;

    let stale = store
        .set(&tag, test_record(&format!("sha256:{}", "a".repeat(64)))?, None)
        .await?;
    store
        .set(&tag, test_record(&format!("sha256:{}", "b".repeat(64)))?, None)
        .await?;

    assert!(matches!(
        store.clear(&tag, stale).await,
        Err(SnapshotTagStoreError::RevisionConflict)
    ));
    let (_, current) = store.get_entry(&tag).await?.ok_or("not found")?;
    store.clear(&tag, current).await?;
    assert!(store.get(&tag).await?.is_none());

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
