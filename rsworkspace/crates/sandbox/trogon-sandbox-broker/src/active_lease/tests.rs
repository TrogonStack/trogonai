use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use trogon_sandbox_contract::{
    LeaseId, LeaseRecord, LeaseStatus, OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts,
    SandboxTags, TenantId, WorkspaceSource,
};

use super::*;
use crate::generated_ulid::generated_ulid;

type TestResult = Result<(), Box<dyn Error>>;

const CPU_SECONDS: u32 = 60;
const MEMORY_MIB: u32 = 512;

/// A minimal valid spec booting `ubuntu:24.04`.
pub(crate) fn sample_spec() -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: std::num::NonZeroU32::new(CPU_SECONDS).ok_or("zero cpu")?,
        memory_mib: std::num::NonZeroU32::new(MEMORY_MIB).ok_or("zero memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?)
}

pub(crate) fn sample_record(id: &str, tenant: &str, deadline: SystemTime) -> Result<LeaseRecord, Box<dyn Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new(id),
        tenant: TenantId::new(tenant)?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline,
        spec: sample_spec()?,
        sandbox_id: format!("sandbox-{id}"),
        cpu_seconds_consumed: CPU_SECONDS,
        wall_seconds_consumed: 300,
        correlation_id: None,
        tags: SandboxTags::default(),
        name: None,
        worker_id: None,
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: None,
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

#[tokio::test]
async fn touch_sets_last_activity_and_never_moves_it_back() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let record = sample_record("lease-touch", "t", SystemTime::now())?;
    store.put(&record).await?;
    let later = SystemTime::now() + Duration::from_secs(10);
    let earlier = SystemTime::now() - Duration::from_secs(10);

    assert!(store.touch(&record.lease_id, later).await?);
    assert!(store.touch(&record.lease_id, earlier).await?);
    let got = store.get(&record.lease_id).await?.ok_or("record missing")?;
    assert_eq!(got.last_activity_at, Some(later));
    Ok(())
}

#[tokio::test]
async fn touch_reports_false_for_a_missing_record() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let missing = LeaseId::new("gone");
    assert!(!store.touch(&missing, SystemTime::now()).await?);
    Ok(())
}

#[tokio::test]
async fn replace_if_unchanged_swaps_only_an_unchanged_record() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let record = sample_record("lease-cas", "t", SystemTime::now())?;
    store.put(&record).await?;
    let mut next = record.clone();
    next.cpu_seconds_consumed = 7;

    assert!(store.replace_if_unchanged(&record, &next).await?);
    // The stored record is now `next`, so a second swap from `record` is stale.
    assert!(!store.replace_if_unchanged(&record, &next).await?);
    let got = store.get(&record.lease_id).await?.ok_or("record missing")?;
    assert_eq!(got.cpu_seconds_consumed, 7);
    let missing = sample_record("lease-missing", "t", SystemTime::now())?;
    assert!(!store.replace_if_unchanged(&missing, &missing).await?);
    Ok(())
}

#[tokio::test]
async fn replace_if_unchanged_refuses_a_record_with_another_lease_id() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let record = sample_record("lease-one", "t", SystemTime::now())?;
    let other = sample_record("lease-two", "t", SystemTime::now())?;
    store.put(&record).await?;
    assert!(store.replace_if_unchanged(&record, &other).await.is_err());
    Ok(())
}

#[tokio::test]
async fn nats_replace_if_unchanged_swaps_only_an_unchanged_record() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let store = NatsKvActiveLeaseStore::new(js, "ACTIVE_LEASE_CAS_TEST").await?;
    let record = sample_record("lease-cas-nats", "t", SystemTime::now())?;
    store.put(&record).await?;
    let mut next = record.clone();
    next.cpu_seconds_consumed = 7;

    assert!(store.replace_if_unchanged(&record, &next).await?);
    assert!(!store.replace_if_unchanged(&record, &next).await?);
    let got = store.get(&record.lease_id).await?.ok_or("record missing")?;
    assert_eq!(got.cpu_seconds_consumed, 7);
    Ok(())
}

#[tokio::test]
async fn nats_touch_keeps_concurrent_field_changes() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let store = NatsKvActiveLeaseStore::new(js, "ACTIVE_LEASE_TOUCH_TEST").await?;
    let record = sample_record("lease-touch-nats", "t", SystemTime::now())?;
    store.put(&record).await?;
    let at = SystemTime::now() + Duration::from_secs(5);

    assert!(store.touch(&record.lease_id, at).await?);
    let got = store.get(&record.lease_id).await?.ok_or("record missing")?;
    assert_eq!(got.last_activity_at, Some(at));
    assert_eq!(got.sandbox_id, record.sandbox_id);
    assert!(!store.touch(&LeaseId::new("gone"), at).await?);
    Ok(())
}

#[tokio::test]
async fn put_and_get_roundtrip() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let lease_id = LeaseId::new("lease-1");
    let record = sample_record("lease-1", "tenant-a", SystemTime::now() + Duration::from_secs(300))?;

    store.put(&record).await?;
    let got = store.get(&lease_id).await?.ok_or("record missing")?;
    assert_eq!(got.lease_id, lease_id);
    Ok(())
}

#[tokio::test]
async fn delete_returns_true_when_present() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let lease_id = LeaseId::new("lease-del");
    let record = sample_record("lease-del", "tenant-a", SystemTime::now() + Duration::from_secs(300))?;

    store.put(&record).await?;
    assert!(store.delete(&lease_id).await?);
    assert!(store.get(&lease_id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn delete_returns_false_when_absent() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    assert!(!store.delete(&LeaseId::new("nonexistent")).await?);
    Ok(())
}

#[tokio::test]
async fn second_delete_is_noop() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let lease_id = LeaseId::new("lease-idem");
    let record = sample_record("lease-idem", "tenant-a", SystemTime::now() + Duration::from_secs(300))?;

    store.put(&record).await?;
    assert!(store.delete(&lease_id).await?);
    assert!(!store.delete(&lease_id).await?);
    Ok(())
}

#[tokio::test]
async fn all_returns_all_stored_records() -> TestResult {
    let store = InMemoryActiveLeaseStore::new();
    let first = sample_record("lease-a1", "t1", SystemTime::now() + Duration::from_secs(10))?;
    let second = sample_record("lease-a2", "t2", SystemTime::now() + Duration::from_secs(20))?;
    store.put(&first).await?;
    store.put(&second).await?;

    assert_eq!(store.all().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn nats_kv_put_get_delete_roundtrip() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let bucket = format!("sandbox-active-leases-test-{}", generated_ulid());
    let store = NatsKvActiveLeaseStore::new(js, &bucket).await?;

    let lease_id = LeaseId::new("nats-lease-1");
    let record = sample_record("nats-lease-1", "tenant-x", SystemTime::now() + Duration::from_secs(300))?;

    store.put(&record).await?;
    assert!(store.get(&lease_id).await?.is_some());
    assert!(store.delete(&lease_id).await?);
    assert!(store.get(&lease_id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn nats_all_lists_every_stored_record() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let store = NatsKvActiveLeaseStore::new(js, "ACTIVE_LEASE_ALL_TEST").await?;
    let first = sample_record("lease-all-1", "t", SystemTime::now())?;
    let second = sample_record("lease-all-2", "t", SystemTime::now())?;
    store.put(&first).await?;
    store.put(&second).await?;

    assert_eq!(store.all().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn nats_delete_has_exactly_one_winner_among_concurrent_callers() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = jetstream::new(server.client.clone());
    let store = Arc::new(NatsKvActiveLeaseStore::new(js, "ACTIVE_LEASE_DELETE_TEST").await?);
    let record = sample_record("lease-cas", "t", SystemTime::now())?;
    store.put(&record).await?;

    let mut deleters = Vec::new();
    for _ in 0..8 {
        let store = Arc::clone(&store);
        let id = record.lease_id.clone();
        deleters.push(tokio::spawn(async move { store.delete(&id).await }));
    }
    let mut winners = 0;
    for deleter in deleters {
        if deleter.await?? {
            winners += 1;
        }
    }
    assert_eq!(winners, 1);
    assert!(store.get(&record.lease_id).await?.is_none());
    Ok(())
}
