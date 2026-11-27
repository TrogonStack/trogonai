use async_nats::jetstream::{self, kv};
use trogon_sandbox_contract::{DriveId, DriveRecord, LeaseId, TenantId};
use trogon_sandbox_protocol::subjects::DRIVE_KV_BUCKET;

use super::NatsKvDriveStore;
use super::drive_keys::legacy_drive_kv_key;
use super::{DriveStore, DriveStoreError, drive_kv_key};
use crate::constants::DRIVE_KV_HISTORY;

async fn make_nats_store(bucket_suffix: &str) -> Result<NatsKvDriveStore, Box<dyn std::error::Error>> {
    let client = async_nats::connect("nats://127.0.0.1:4222").await?;
    let js = async_nats::jetstream::new(client);
    let kv = js
        .create_key_value(kv::Config {
            bucket: format!("{DRIVE_KV_BUCKET}-{bucket_suffix}"),
            history: DRIVE_KV_HISTORY,
            ..Default::default()
        })
        .await?;
    Ok(NatsKvDriveStore { kv })
}

async fn cleanup_test_bucket(bucket: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = async_nats::connect("nats://127.0.0.1:4222").await?;
    let js = async_nats::jetstream::new(client);
    let _ = js.delete_key_value(bucket).await;
    Ok(())
}

fn test_drive_id(tenant: &str, name: &str) -> Result<DriveId, Box<dyn std::error::Error>> {
    Ok(DriveId {
        tenant: TenantId::new(tenant)?,
        name: trogon_sandbox_contract::DriveName::new(name)?,
    })
}
#[tokio::test]
async fn a_claim_release_guarded_by_revision_skips_a_rewritten_claim() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvDriveStore::new(jetstream::new(server.client.clone())).await?;
    let tenant = TenantId::new("acme")?;
    let drive = test_drive_id("acme", "data")?;
    store.create(&tenant, &drive, 1, "bucket".into()).await?;
    let lease = LeaseId::new("lease-a");
    assert!(store.list_claims(None, 10).await?.claims.is_empty());
    store.set_attached_lease(&drive, Some(lease.clone())).await?;
    let claims = store.list_claims(None, 10).await?.claims;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].lease_id, lease);
    assert!(claims[0].claimed_at.is_some());
    // The claim is re-written between the listing and the release.
    store.set_attached_lease(&drive, None).await?;
    store.set_attached_lease(&drive, Some(lease.clone())).await?;
    assert!(!store.release_claim_at(&drive, &lease, claims[0].revision).await?);
    let fresh = store.list_claims(None, 10).await?.claims;
    assert!(store.release_claim_at(&drive, &lease, fresh[0].revision).await?);
    assert!(store.list_claims(None, 10).await?.claims.is_empty());
    Ok(())
}
#[tokio::test]
async fn listing_does_not_confuse_tenant_prefixes_or_normalized_names() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvDriveStore::new(jetstream::new(server.client.clone())).await?;
    let owner = TenantId::new("acme")?;
    let prefixed = TenantId::new("acme_team")?;
    store
        .create(&owner, &test_drive_id("acme", "data")?, 1, "bucket".into())
        .await?;
    store
        .create(&prefixed, &test_drive_id("acme_team", "data")?, 1, "bucket".into())
        .await?;
    let drives = store.list(&owner).await?;
    assert_eq!(drives.len(), 1, "{drives:?}");
    assert_eq!(drives[0].tenant, owner);
    Ok(())
}
#[tokio::test]
async fn colliding_legacy_pairs_get_distinct_keys_and_independent_drives() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvDriveStore::new(jetstream::new(server.client.clone())).await?;
    let actual = test_drive_id("acme_team", "data")?;
    let collision = test_drive_id("acme", "team_data")?;
    assert_eq!(legacy_drive_kv_key(&actual), legacy_drive_kv_key(&collision));
    assert_ne!(drive_kv_key(&actual), drive_kv_key(&collision));
    store.create(&actual.tenant, &actual, 1, "bucket".into()).await?;
    assert!(store.get(&collision).await?.is_none());
    assert!(matches!(
        store.set_attached_lease(&collision, Some(LeaseId::new("wrong"))).await,
        Err(DriveStoreError::NotFound)
    ));
    assert!(matches!(store.delete(&collision).await, Err(DriveStoreError::NotFound)));
    store.create(&collision.tenant, &collision, 2, "bucket".into()).await?;
    assert_eq!(store.get(&actual).await?.ok_or("missing drive")?.size_gib, 1);
    assert_eq!(store.get(&collision).await?.ok_or("missing drive")?.size_gib, 2);
    assert!(
        store
            .get(&actual)
            .await?
            .ok_or("missing drive")?
            .attached_lease
            .is_none()
    );
    Ok(())
}
#[tokio::test]
async fn legacy_keyed_drive_stays_reachable_only_by_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvDriveStore::new(jetstream::new(server.client.clone())).await?;
    let owner = test_drive_id("acme_team", "data")?;
    let other = test_drive_id("acme", "team_data")?;
    let legacy = DriveRecord {
        id: owner.clone(),
        tenant: owner.tenant.clone(),
        size_gib: 3,
        created_at: std::time::SystemTime::now(),
        attached_lease: None,
        storage: trogon_sandbox_contract::DriveStorage::S3 {
            bucket: "bucket".into(),
        },
    };
    store
        .kv
        .put(legacy_drive_kv_key(&owner), serde_json::to_vec(&legacy)?.into())
        .await?;

    assert_eq!(store.get(&owner).await?.ok_or("missing drive")?.size_gib, 3);
    assert!(store.get(&other).await?.is_none());
    assert!(matches!(
        store.create(&owner.tenant, &owner, 1, "bucket".into()).await,
        Err(DriveStoreError::AlreadyExists)
    ));
    assert_eq!(store.list(&owner.tenant).await?.len(), 1);
    assert!(store.list(&other.tenant).await?.is_empty());

    let lease = LeaseId::new("lease-1");
    store.set_attached_lease(&owner, Some(lease.clone())).await?;
    assert_eq!(
        store.get(&owner).await?.ok_or("missing drive")?.attached_lease,
        Some(lease.clone())
    );
    assert!(matches!(
        store.release_if_attached(&other, &lease).await,
        Err(DriveStoreError::NotFound)
    ));
    assert!(!store.release_if_attached(&owner, &LeaseId::new("lease-2")).await?);
    assert!(store.release_if_attached(&owner, &lease).await?);
    store.delete(&owner).await?;
    assert!(store.get(&owner).await?.is_none());
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn create_succeeds_and_returns_record_with_correct_size() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let tenant = TenantId::new("acme")?;
    let drive_id = test_drive_id("acme", "data0")?;

    let record = store.create(&tenant, &drive_id, 50, "test-bucket".to_string()).await?;
    assert_eq!(record.size_gib, 50);
    assert_eq!(record.id, drive_id);
    assert_eq!(record.tenant, tenant);
    assert_eq!(record.storage.s3_bucket(), Some("test-bucket"));
    assert!(record.attached_lease.is_none());

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn create_fails_with_already_exists_on_duplicate_id() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let tenant = TenantId::new("acme")?;
    let drive_id = test_drive_id("acme", "data0")?;

    store.create(&tenant, &drive_id, 50, "test-bucket".to_string()).await?;
    let err = store
        .create(&tenant, &drive_id, 60, "test-bucket".to_string())
        .await
        .err()
        .ok_or("duplicate create must fail")?;
    assert!(matches!(err, DriveStoreError::AlreadyExists));

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn get_returns_none_for_missing() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let drive_id = test_drive_id("acme", "missing")?;

    assert!(store.get(&drive_id).await?.is_none());

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn list_returns_only_querying_tenant_drives() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let tenant_a = TenantId::new("acme")?;
    let tenant_b = TenantId::new("globex")?;

    store
        .create(&tenant_a, &test_drive_id("acme", "d0")?, 10, "test-bucket".to_string())
        .await?;
    store
        .create(&tenant_a, &test_drive_id("acme", "d1")?, 20, "test-bucket".to_string())
        .await?;
    store
        .create(
            &tenant_b,
            &test_drive_id("globex", "d0")?,
            30,
            "test-bucket".to_string(),
        )
        .await?;

    let drives = store.list(&tenant_a).await?;
    assert_eq!(drives.len(), 2);
    assert!(drives.iter().all(|d| d.tenant == tenant_a));

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn delete_removes_entry_so_subsequent_get_returns_none() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let tenant = TenantId::new("acme")?;
    let drive_id = test_drive_id("acme", "data0")?;

    store.create(&tenant, &drive_id, 50, "test-bucket".to_string()).await?;
    store.delete(&drive_id).await?;
    assert!(store.get(&drive_id).await?.is_none());

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn delete_fails_on_missing_id() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let drive_id = test_drive_id("acme", "missing")?;

    let err = store.delete(&drive_id).await.err().ok_or("must fail")?;
    assert!(matches!(err, DriveStoreError::NotFound));

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn set_attached_lease_updates_record() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let tenant = TenantId::new("acme")?;
    let drive_id = test_drive_id("acme", "data0")?;

    store.create(&tenant, &drive_id, 50, "test-bucket".to_string()).await?;

    let lease_id = LeaseId::new("lease-abc");
    store.set_attached_lease(&drive_id, Some(lease_id.clone())).await?;
    let record = store.get(&drive_id).await?.ok_or("missing drive")?;
    assert_eq!(record.attached_lease, Some(lease_id));

    store.set_attached_lease(&drive_id, None).await?;
    let record = store.get(&drive_id).await?.ok_or("missing drive")?;
    assert!(record.attached_lease.is_none());

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
#[tokio::test]
#[ignore = "requires nats-server with JetStream on PATH"]
async fn list_claims_pages_over_attached_drives_only() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = crate::generated_ulid::generated_ulid();
    let bucket = format!("{DRIVE_KV_BUCKET}-{suffix}");
    let store = make_nats_store(&suffix).await?;
    let tenant = TenantId::new("acme")?;

    let mut drives: Vec<DriveId> = (0..10)
        .map(|n| test_drive_id("acme", &format!("d{n}")))
        .collect::<Result<_, _>>()?;
    drives.sort_by_key(drive_kv_key);
    for drive in &drives {
        store.create(&tenant, drive, 1, "test-bucket".to_string()).await?;
    }
    // The first three keys carry no claim, so the first page at limit 2
    // filters out every key it examined while more remain.
    let lease = LeaseId::new("lease-paging");
    for idx in [3, 4, 7, 8] {
        store.set_attached_lease(&drives[idx], Some(lease.clone())).await?;
    }
    store.delete(&drives[8]).await?;
    let claimed: Vec<&DriveId> = [3, 4, 7].iter().map(|&idx| &drives[idx]).collect();

    let first = store.list_claims(None, 2).await?;
    assert!(first.claims.is_empty());
    assert_eq!(first.next, Some(drive_kv_key(&drives[1])));

    for limit in [1, 2, 3, 100] {
        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(pages <= 20, "cursor did not terminate at limit {limit}");
            let page = store.list_claims(after.as_deref(), limit).await?;
            assert!(page.claims.len() <= limit);
            seen.extend(page.claims.iter().map(|claim| claim.drive_id.clone()));
            match page.next {
                Some(next) => {
                    assert!(after.as_ref().is_none_or(|prev| next > *prev));
                    after = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(
            seen.iter().collect::<Vec<_>>(),
            claimed,
            "limit {limit}: every claimed drive exactly once, in key order"
        );
    }

    let exclusive = store.list_claims(Some(&drive_kv_key(&drives[3])), 100).await?;
    assert_eq!(
        exclusive.claims.iter().map(|claim| &claim.drive_id).collect::<Vec<_>>(),
        vec![&drives[4], &drives[7]]
    );
    assert!(exclusive.next.is_none());

    cleanup_test_bucket(&bucket).await?;
    Ok(())
}
