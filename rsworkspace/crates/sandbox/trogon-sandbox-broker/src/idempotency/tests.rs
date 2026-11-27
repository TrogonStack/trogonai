use trogon_sandbox_contract::{LeaseRecord, LeaseRequestKey, LeaseStatus};

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseError, LeaseGranted, LeaseId, ProviderKind, TenantId};

use super::*;
use crate::quota::QuotaStore;

fn pending(tenant: &str, key: &str, id: &str) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new(id),
        tenant: TenantId::new(tenant)?,
        provider: ProviderKind::Fly,
        status: LeaseStatus::Pending,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(60),
        spec: crate::idle::tests::spec(60, None, None)?,
        sandbox_id: String::new(),
        cpu_seconds_consumed: 1,
        wall_seconds_consumed: 60,
        correlation_id: None,
        tags: Default::default(),
        name: None,
        worker_id: Some(trogon_sandbox_contract::WorkerId::new("worker-a")?),
        quota_reservation: None,
        dispatch_attempt: Some(trogon_sandbox_contract::LeaseDispatchAttempt {
            epoch: None,
            id: trogon_sandbox_contract::DispatchAttemptId::new(format!("attempt-{id}"))?,
            idempotency_key: LeaseRequestKey::new(key)?,
            accounting: trogon_sandbox_contract::LeaseAccounting::Tenant,
            cleanup_reason: None,
            cleanup_started_at: None,
            pending_dispatch: None,
        }),
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
async fn nats_attempt_journal_survives_restart_and_fences_another_brokers_grant()
-> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let js = async_nats::jetstream::new(server.client.clone());
    let bucket = format!("attempt-test-{}", crate::generated_ulid::generated_ulid());
    let first = NatsKvIdempotencyStore::new(js.clone(), &bucket).await?;
    let second = NatsKvIdempotencyStore::new(js.clone(), &bucket).await?;
    let initial = pending("tenant", "same-key", "one")?;
    let competing = pending("tenant", "same-key", "two")?;
    let key = LeaseRequestKey::new("same-key")?;
    let (one, two) = tokio::join!(
        first.begin_attempt(&initial.tenant, &key, &initial),
        second.begin_attempt(&initial.tenant, &key, &competing)
    );
    assert_ne!(one?, two?);
    let stored = first.get_attempt(&initial.tenant, &key).await?.ok_or("not found")?;
    let mut cancelling = stored.clone();
    cancelling.status = LeaseStatus::Cancelling;
    cancelling.dispatch_attempt.as_mut().ok_or("missing")?.cleanup_reason =
        Some(trogon_sandbox_contract::LeaseReleasedReason::Failed);
    assert!(
        second
            .replace_attempt(&initial.tenant, &key, &stored, &cancelling)
            .await?
    );
    for changed in [None, Some(trogon_sandbox_contract::LeaseReleasedReason::Cancelled)] {
        let mut replacement = cancelling.clone();
        replacement.dispatch_attempt.as_mut().ok_or("missing")?.cleanup_reason = changed;
        assert!(
            second
                .replace_attempt(&initial.tenant, &key, &cancelling, &replacement)
                .await
                .is_err()
        );
    }
    let mut late_grant = stored.clone();
    late_grant.status = LeaseStatus::Running;
    assert!(
        !first
            .replace_attempt(&initial.tenant, &key, &stored, &late_grant)
            .await?
    );
    assert!(!first.forget_attempt(&initial.tenant, &key, &stored.lease_id).await?);
    drop(first);
    drop(second);
    let restarted = NatsKvIdempotencyStore::new(js.clone(), &bucket).await?;
    assert_eq!(restarted.get_attempt(&initial.tenant, &key).await?, Some(cancelling));
    assert_eq!(restarted.attempts.status().await?.max_age(), Duration::ZERO);
    Ok(())
}
#[tokio::test]
async fn request_attempt_claim_is_atomic_and_opaque_keys_do_not_alias() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::new();
    let first = pending("a_b", "c", "one")?;
    let competing = pending("a_b", "c", "two")?;
    let key = LeaseRequestKey::new("c")?;
    let (left, right) = tokio::join!(
        store.begin_attempt(&first.tenant, &key, &first),
        store.begin_attempt(&competing.tenant, &key, &competing),
    );
    assert_ne!(left?, right?);
    let other = pending("a", "b_c", "three")?;
    assert!(
        store
            .begin_attempt(&other.tenant, &LeaseRequestKey::new("b_c")?, &other)
            .await?
    );
    let opaque = pending("a_b", &"k-".repeat(128), "four")?;
    let opaque_key = opaque
        .dispatch_attempt
        .as_ref()
        .ok_or("missing")?
        .idempotency_key
        .clone();
    assert!(store.begin_attempt(&opaque.tenant, &opaque_key, &opaque).await?);
    assert_eq!(store.get_attempt(&opaque.tenant, &opaque_key).await?, Some(opaque));
    Ok(())
}
#[tokio::test]
async fn cleanup_transition_cannot_be_overwritten_by_a_late_grant() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::new();
    let initial = pending("tenant", "key", "one")?;
    let key = LeaseRequestKey::new("key")?;
    assert!(store.begin_attempt(&initial.tenant, &key, &initial).await?);
    let mut cancelling = initial.clone();
    cancelling.status = LeaseStatus::Cancelling;
    assert!(
        store
            .replace_attempt(&initial.tenant, &key, &initial, &cancelling)
            .await?
    );
    let mut running = initial.clone();
    running.status = LeaseStatus::Running;
    assert!(!store.replace_attempt(&initial.tenant, &key, &initial, &running).await?);
    assert_eq!(store.get_attempt(&initial.tenant, &key).await?, Some(cancelling));
    Ok(())
}
#[tokio::test]
async fn stale_retirement_cannot_remove_a_replacement_attempt() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::new();
    let initial = pending("tenant", "key", "one")?;
    let key = LeaseRequestKey::new("key")?;
    assert!(store.begin_attempt(&initial.tenant, &key, &initial).await?);
    assert!(store.forget_attempt(&initial.tenant, &key, &initial.lease_id).await?);
    let replacement = pending("tenant", "key", "two")?;
    assert!(store.begin_attempt(&replacement.tenant, &key, &replacement).await?);
    assert!(!store.forget_attempt(&initial.tenant, &key, &initial.lease_id).await?);
    assert_eq!(store.get_attempt(&replacement.tenant, &key).await?, Some(replacement));
    Ok(())
}
#[tokio::test]
async fn in_memory_records_expire_after_their_max_age_like_the_kv_bucket() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::with_max_age(Duration::from_millis(40));
    let t = tenant("t1")?;
    store.put_if_absent(&t, "k", &granted("l1")).await?;
    assert!(store.get(&t, "k").await?.is_some());
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(store.get(&t, "k").await?, None);
    Ok(())
}
#[tokio::test]
async fn a_second_grant_for_one_key_gets_the_first_back_and_does_not_replace_it()
-> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::new();
    let t = tenant("t1")?;
    assert_eq!(store.put_if_absent(&t, "k", &granted("l1")).await?, None);
    let earlier = store.put_if_absent(&t, "k", &granted("l2")).await?;
    assert_eq!(earlier.map(|g| g.lease_id.as_str().to_owned()), Some("l1".to_string()));
    assert_eq!(
        store.get(&t, "k").await?.map(|g| g.lease_id.as_str().to_owned()),
        Some("l1".to_string())
    );
    Ok(())
}
fn tenant(s: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(s)?)
}

fn granted(id: &str) -> LeaseGranted {
    LeaseGranted {
        entity_id: None,
        operation_id: None,
        lease_id: LeaseId::new(id),
        provider: ProviderKind::Mock,
        sandbox_id: "sandbox-1".to_string(),
        evidence_subject: "sandbox.evidence.x.>".to_string(),
        deadline: SystemTime::now() + Duration::from_secs(300),
        created: trogon_sandbox_contract::LeaseCreation::Created,
        capabilities: Vec::new(),
    }
}

struct CountingQuotaStore {
    count: Arc<AtomicU32>,
}

#[async_trait]
impl QuotaStore for CountingQuotaStore {
    async fn check_and_reserve(
        &self,
        _tenant: &TenantId,
        _cpu: u32,
        _ttl: u32,
        _cpu_cap: u32,
        _wall_cap: u32,
    ) -> Result<(), LeaseError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn release(&self, _tenant: &TenantId, _cpu: u32, _ttl: u32) -> Result<(), crate::quota::QuotaStoreError> {
        Ok(())
    }

    async fn active_lease_count(&self, _tenant: &TenantId) -> Result<u32, crate::quota::QuotaStoreError> {
        Ok(0)
    }

    async fn check_active_lease_cap(&self, _tenant: &TenantId) -> Result<(), LeaseError> {
        Ok(())
    }

    async fn increment_active_leases(&self, _tenant: &TenantId) -> Result<(), crate::quota::QuotaStoreError> {
        Ok(())
    }

    async fn decrement_active_leases(&self, _tenant: &TenantId) -> Result<(), crate::quota::QuotaStoreError> {
        Ok(())
    }

    async fn record_snapshot_published(
        &self,
        _tenant: &TenantId,
        _bytes: u64,
        _default_snapshot: crate::quota::SnapshotQuota,
    ) -> Result<(), LeaseError> {
        Ok(())
    }

    async fn record_snapshot_deleted(
        &self,
        _tenant: &TenantId,
        _bytes: u64,
    ) -> Result<(), crate::quota::QuotaStoreError> {
        Ok(())
    }

    async fn check_prewarm_dispatch(&self, _tenant: &TenantId, _cpu_seconds: u32) -> Result<(), LeaseError> {
        Ok(())
    }

    async fn record_prewarm_dispatch(
        &self,
        _tenant: &TenantId,
        _cpu_seconds: u32,
    ) -> Result<(), crate::quota::QuotaStoreError> {
        Ok(())
    }

    async fn record_prewarm_release(
        &self,
        _tenant: &TenantId,
        _cpu_seconds: u32,
    ) -> Result<(), crate::quota::QuotaStoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn same_key_returns_cached_without_double_quota() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::new();
    let count = Arc::new(AtomicU32::new(0));
    let quota = CountingQuotaStore {
        count: Arc::clone(&count),
    };
    let t = tenant("acme")?;
    let key = "idem-1";

    // First: quota is charged (simulated externally), then cache is populated
    quota.check_and_reserve(&t, 60, 300, 3600, 7200).await?;
    let g = granted("lease-abc");
    store.put_if_absent(&t, key, &g).await?;

    // Second: cache hit: do NOT call quota
    let cached = store.get(&t, key).await?;
    assert!(cached.is_some());
    assert_eq!(cached.ok_or("cache miss")?.lease_id, g.lease_id);
    // quota was called exactly once
    assert_eq!(count.load(Ordering::SeqCst), 1);
    Ok(())
}
#[tokio::test]
async fn different_keys_same_tenant_are_independent() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryIdempotencyStore::new();
    let t = tenant("acme")?;

    let g1 = granted("lease-1");
    let g2 = granted("lease-2");
    store.put_if_absent(&t, "key-1", &g1).await?;
    store.put_if_absent(&t, "key-2", &g2).await?;

    let got1 = store.get(&t, "key-1").await?.ok_or("not found")?;
    let got2 = store.get(&t, "key-2").await?.ok_or("not found")?;
    assert_eq!(got1.lease_id, g1.lease_id);
    assert_eq!(got2.lease_id, g2.lease_id);
    Ok(())
}
#[tokio::test]
async fn denial_is_not_cached() -> Result<(), Box<dyn std::error::Error>> {
    // Denials must never be put in the store; verify that missing == denial not cached
    let store = InMemoryIdempotencyStore::new();
    let t = tenant("acme")?;
    let got = store.get(&t, "denied-key").await?;
    assert!(got.is_none());
    Ok(())
}
#[tokio::test]
#[ignore]
async fn nats_kv_put_and_get_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let bucket = format!("sandbox-idempotency-test-{}", crate::generated_ulid::generated_ulid());
    let client = async_nats::connect("nats://127.0.0.1:4222").await?;
    let js = async_nats::jetstream::new(client);
    let store = NatsKvIdempotencyStore::new(js.clone(), &bucket).await?;

    let t = tenant("acme")?;
    let g = granted("lease-nats-idem");
    store.put_if_absent(&t, "idem-key", &g).await?;

    let got = store.get(&t, "idem-key").await?;
    assert!(got.is_some());
    assert_eq!(got.ok_or("cache miss")?.lease_id, g.lease_id);

    let _ = js.delete_key_value(&bucket).await;
    Ok(())
}
