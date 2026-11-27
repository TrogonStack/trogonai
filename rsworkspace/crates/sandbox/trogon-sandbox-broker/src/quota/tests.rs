use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use chrono::{TimeZone, Utc};
use trogon_sandbox_contract::{DispatchAttemptId, LeaseAccounting, LeaseError, LeaseId, TenantId};

use super::attempt_budget::{attempt_month_key, new_attempt_record, release_attempt_budget, reserve_attempt_budget};
use super::attempt_quota_receipt::{AttemptQuotaPhase, AttemptQuotaReceipt};
use super::quota_keys::nats_kv_keys;
use super::*;

fn reservation(id: &str) -> Result<AttemptQuotaReservation, Box<dyn std::error::Error>> {
    Ok(AttemptQuotaReservation {
        tenant: TenantId::new("atomic-tenant")?,
        attempt_id: DispatchAttemptId::new(id)?,
        created_at: SystemTime::now(),
        cpu_seconds: 3,
        wall_seconds: 7,
        accounting: LeaseAccounting::Tenant,
    })
}
fn attempt_limits() -> AttemptQuotaLimits {
    AttemptQuotaLimits {
        cpu: 30,
        wall: 70,
        active: 2,
        max_lease_lifetime_seconds: crate::constants::MAX_LEASE_TTL_SECONDS,
    }
}

#[tokio::test]
async fn held_attempt_window_follows_the_configured_lifetime_cap() -> Result<(), Box<dyn std::error::Error>> {
    let day = u64::from(crate::constants::MAX_LEASE_TTL_SECONDS);
    let mut old = reservation("long-lived")?;
    old.created_at = SystemTime::now() - Duration::from_secs(day * 3);

    let store = InMemoryQuotaStore::new();
    assert!(
        store.reserve_attempt(&old, &attempt_limits()).await.is_err(),
        "the default cap keeps the two-day window"
    );

    let store = InMemoryQuotaStore::new();
    let limits = AttemptQuotaLimits {
        max_lease_lifetime_seconds: crate::constants::MAX_LEASE_TTL_SECONDS * 2,
        ..attempt_limits()
    };
    store.reserve_attempt(&old, &limits).await?;
    Ok(())
}
#[tokio::test]
async fn attempt_reservation_and_refund_are_exactly_once_and_identity_bound() -> Result<(), Box<dyn std::error::Error>>
{
    let store = InMemoryQuotaStore::new();
    let reservation = reservation("once")?;
    let limits = attempt_limits();
    store.reserve_attempt(&reservation, &limits).await?;
    store.reserve_attempt(&reservation, &limits).await?;
    assert_eq!(store.active_lease_count(&reservation.tenant).await?, 1);
    let mut wrong = reservation.clone();
    wrong.cpu_seconds += 1;
    assert!(store.release_attempt(&wrong, &limits).await.is_err());
    assert!(store.release_attempt(&reservation, &limits).await?);
    assert!(!store.release_attempt(&reservation, &limits).await?);
    assert!(store.reserve_attempt(&reservation, &limits).await.is_err());
    let records = store.records.lock().await;
    let record = &records[&attempt_month_key(&reservation)?];
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (0, 0, 0)
    );
    Ok(())
}
#[tokio::test]
async fn attempt_release_before_reservation_prevents_a_late_charge() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let reservation = reservation("cancel-first")?;
    let limits = attempt_limits();
    assert!(!store.release_attempt(&reservation, &limits).await?);
    assert!(store.reserve_attempt(&reservation, &limits).await.is_err());
    assert_eq!(store.active_lease_count(&reservation.tenant).await?, 0);
    Ok(())
}
#[tokio::test]
async fn concurrent_attempts_enforce_the_active_cap_in_the_reservation_transaction()
-> Result<(), Box<dyn std::error::Error>> {
    let store = std::sync::Arc::new(InMemoryQuotaStore::new());
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for id in ["left", "right"] {
        let store = store.clone();
        let barrier = barrier.clone();
        let reservation = reservation(id)?;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            store
                .reserve_attempt(
                    &reservation,
                    &AttemptQuotaLimits {
                        active: 1,
                        ..attempt_limits()
                    },
                )
                .await
        }));
    }
    let left = tasks.remove(0).await?;
    let right = tasks.remove(0).await?;
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(store.active_lease_count(&TenantId::new("atomic-tenant")?).await?, 1);
    Ok(())
}
#[tokio::test]
async fn prewarm_first_preserves_its_dedicated_count_budget() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let limits = AttemptQuotaLimits {
        cpu: 10_000,
        wall: 20_000,
        active: 100,
        max_lease_lifetime_seconds: crate::constants::MAX_LEASE_TTL_SECONDS,
    };
    for index in 0..PrewarmQuota::default().count_cap {
        let mut attempt = reservation(&format!("prewarm-first-{index}"))?;
        attempt.accounting = LeaseAccounting::Prewarm;
        store.reserve_attempt(&attempt, &limits).await?;
    }
    let mut overflow = reservation("prewarm-overflow")?;
    overflow.accounting = LeaseAccounting::Prewarm;
    assert!(matches!(
        store.reserve_attempt(&overflow, &limits).await,
        Err(LeaseError::PrewarmQuotaExceeded { .. })
    ));
    let ordinary = reservation("ordinary-after-prewarm")?;
    store.reserve_attempt(&ordinary, &limits).await?;
    assert_eq!(store.active_lease_count(&ordinary.tenant).await?, 1);
    Ok(())
}
#[tokio::test]
async fn prewarm_attempts_never_debit_or_refund_ordinary_tenant_budget() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let mut reservation = reservation("prewarm")?;
    reservation.accounting = LeaseAccounting::Prewarm;
    let limits = attempt_limits();
    store.reserve_attempt(&reservation, &limits).await?;
    {
        let records = store.records.lock().await;
        let record = &records[&attempt_month_key(&reservation)?];
        assert_eq!(
            (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
            (0, 0, 0)
        );
        assert_eq!((record.prewarm.active_count, record.prewarm.total_cpu_seconds), (1, 3));
    }
    assert!(store.release_attempt(&reservation, &limits).await?);
    let records = store.records.lock().await;
    let record = &records[&attempt_month_key(&reservation)?];
    assert_eq!((record.prewarm.active_count, record.prewarm.total_cpu_seconds), (0, 0));
    Ok(())
}
#[tokio::test]
async fn delayed_release_refunds_the_original_month() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let mut reservation = reservation("prior-month")?;
    let old_date = Utc
        .with_ymd_and_hms(2020, 1, 31, 23, 59, 59)
        .single()
        .ok_or("invalid date")?;
    reservation.created_at = old_date.into();
    let limits = attempt_limits();
    let mut record = new_attempt_record(&limits);
    reserve_attempt_budget(&mut record, &reservation)?;
    store
        .records
        .lock()
        .await
        .insert(attempt_month_key(&reservation)?, record);
    store.attempts.lock().await.insert(
        reservation.attempt_id.clone(),
        AttemptQuotaReceipt {
            reservation: reservation.clone(),
            phase: AttemptQuotaPhase::Held,
        },
    );
    store.check_and_reserve(&reservation.tenant, 5, 11, 30, 70).await?;
    assert!(store.release_attempt(&reservation, &limits).await?);
    let records = store.records.lock().await;
    let current = &records[&quota_key(&reservation.tenant)];
    let old = &records[&attempt_month_key(&reservation)?];
    assert_eq!((current.cpu_seconds_used, current.wall_seconds_used), (5, 11));
    assert_eq!(
        (old.cpu_seconds_used, old.wall_seconds_used, old.active_leases),
        (0, 0, 0)
    );
    Ok(())
}
#[test]
fn quota_kv_keys_never_collide_across_tenants() -> Result<(), Box<dyn std::error::Error>> {
    let tenants = ["a.b", "a_b", "a:b", "a/b", "ab"];
    let keys: std::collections::HashSet<String> = tenants
        .iter()
        .map(|t| tenant(t).map(|t| quota_kv_key_for_month(&t, 2026, 11)))
        .collect::<Result<_, _>>()?;
    assert_eq!(keys.len(), tenants.len());
    assert_eq!(
        legacy_quota_kv_key_for_month(&tenant("a:b")?, 2026, 11),
        legacy_quota_kv_key_for_month(&tenant("a_b")?, 2026, 11)
    );
    assert_eq!(
        quota_kv_key_for_month(&tenant("acme-1")?, 2026, 3),
        legacy_quota_kv_key_for_month(&tenant("acme-1")?, 2026, 3),
        "plain tenants keep their key"
    );
    Ok(())
}

#[tokio::test]
async fn nats_quota_seeds_the_current_key_from_the_legacy_record_once() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let colon = tenant("a:b")?;
    let underscore = tenant("a_b")?;
    let (_, legacy) = nats_kv_keys(&colon);
    let mut old = QuotaRecord::new(3600, 7200);
    old.consume(10, 20);
    store.kv.create(&legacy, serde_json::to_vec(&old)?.into()).await?;

    store.check_and_reserve(&colon, 5, 5, 3600, 7200).await?;
    store.check_and_reserve(&colon, 5, 5, 3600, 7200).await?;

    let colon_record = store.quota_record(&colon).await?.ok_or("not found")?;
    assert_eq!(
        (colon_record.cpu_seconds_used, colon_record.wall_seconds_used),
        (20, 30),
        "legacy usage carries over once and is not counted twice"
    );
    let untouched: QuotaRecord = serde_json::from_slice(&store.kv.get(&legacy).await?.ok_or("not found")?)?;
    assert_eq!(untouched.cpu_seconds_used, 10, "the legacy record is never written");

    // `a_b` shared the legacy key, so it starts from the shared usage
    // (never less than before the upgrade) and then diverges.
    let underscore_record = store.quota_record(&underscore).await?.ok_or("not found")?;
    assert_eq!(underscore_record.cpu_seconds_used, 10);
    assert_ne!(nats_kv_keys(&colon).0, nats_kv_keys(&underscore).0);

    let dotted = tenant("a.b")?;
    assert!(store.quota_record(&dotted).await?.is_none());
    Ok(())
}
fn holding(cpu: u32, wall: u32) -> QuotaRecord {
    let mut record = QuotaRecord::new(1_000, 1_000);
    record.cpu_seconds_used = cpu;
    record.wall_seconds_used = wall;
    record.active_leases = 2;
    record
}

#[test]
fn receipts_past_the_byte_budget_evict_the_oldest() {
    let mut record = holding(0, 0);
    let long = "x".repeat(1024);
    for i in 0..1_000u64 {
        assert!(record.refund_release_once(&format!("{long}{i}"), 1_000 + i, 0, 0));
    }
    let bytes: usize = record
        .refunded_releases
        .keys()
        .map(|id| id.len() + REFUND_RECEIPT_OVERHEAD_BYTES)
        .sum();
    assert!(bytes <= MAX_REFUND_RECEIPT_BYTES);
    assert!(record.refunded_releases.len() < 1_000);
    assert!(record.refunded_releases.contains_key(&format!("{long}999")));
    assert!(!record.refunded_releases.contains_key(&format!("{long}0")));
}

#[test]
fn receipts_past_the_cap_evict_the_oldest() {
    let mut record = holding(0, 0);
    for i in 0..=MAX_REFUND_RECEIPTS {
        assert!(record.refund_release_once(&format!("lease-{i}"), 1_000 + i as u64, 0, 0));
    }
    assert_eq!(record.refunded_releases.len(), MAX_REFUND_RECEIPTS);
    assert!(!record.refunded_releases.contains_key("lease-0"));
    assert!(record.refunded_releases.contains_key("lease-1"));
}

#[test]
fn a_receipt_makes_the_refund_apply_once() {
    let mut record = holding(60, 80);
    assert!(record.refund_release_once("lease-a", 1_000, 30, 40));
    assert!(!record.refund_release_once("lease-a", 1_001, 30, 40));
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (30, 40, 1)
    );
    assert!(record.refund_release_once("lease-b", 1_002, 30, 40));
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (0, 0, 0)
    );
}

#[test]
fn receipts_older_than_the_retention_are_pruned() {
    let mut record = holding(60, 80);
    let keep = REFUND_RECEIPT_RETENTION.as_secs();
    assert!(record.refund_release_once("old", 1_000, 30, 40));
    assert!(!record.refund_release_once("old", 1_000 + keep - 1, 0, 0));
    assert!(record.refund_release_once("new", 1_000 + keep, 30, 40));
    assert!(!record.refunded_releases.contains_key("old"));
    assert!(record.refunded_releases.contains_key("new"));
}

#[tokio::test]
async fn the_in_memory_store_refunds_a_lease_once() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let tenant = tenant("refund-once")?;
    for _ in 0..2 {
        store.check_and_reserve(&tenant, 30, 40, 1_000, 1_000).await?;
        store.increment_active_leases(&tenant).await?;
    }
    let lease = LeaseId::new("lease-a");
    let at = SystemTime::now();
    assert!(store.refund_release(&tenant, &lease, at, 30, 40).await?);
    assert!(!store.refund_release(&tenant, &lease, at, 30, 40).await?);
    let record = store.quota_record(&tenant).await?.ok_or("not found")?;
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (30, 40, 1)
    );
    Ok(())
}
#[tokio::test]
async fn nats_concurrent_refunds_of_one_lease_apply_once() -> Result<(), Box<dyn std::error::Error>> {
    let (server, store) = local_attempt_store().await?;
    let other = NatsKvQuotaStore::new(jetstream::new(server.client.clone()), "atomic-quota").await?;
    let tenant = tenant("nats-refund")?;
    for _ in 0..2 {
        store.check_and_reserve(&tenant, 30, 40, 1_000, 1_000).await?;
        store.increment_active_leases(&tenant).await?;
    }
    let lease = LeaseId::new("lease-a");
    let at = SystemTime::now();
    let results = futures::future::join_all((0..6).map(|i| {
        let store = if i % 2 == 0 { &store } else { &other };
        let (tenant, lease) = (&tenant, &lease);
        async move {
            store
                .refund_release(tenant, lease, at, 30, 40)
                .await
                .is_ok_and(|applied| applied)
        }
    }))
    .await;
    assert_eq!(results.iter().filter(|applied| **applied).count(), 1);
    let record = store.quota_record(&tenant).await?.ok_or("not found")?;
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (30, 40, 1)
    );
    // A redo with the marker's timestamp lands on the same month record.
    assert!(!other.refund_release(&tenant, &lease, at, 30, 40).await?);
    Ok(())
}
async fn local_attempt_store() -> Result<(crate::test_support::LocalNats, NatsKvQuotaStore), Box<dyn std::error::Error>>
{
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvQuotaStore::new(jetstream::new(server.client.clone()), "atomic-quota").await?;
    Ok((server, store))
}

#[tokio::test]
async fn nats_attempts_recover_interrupted_budget_and_receipt_writes() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let reservation = reservation("interrupted")?;
    let limits = attempt_limits();
    let key = attempt_month_key(&reservation)?.replace(':', "_");
    let mut record = new_attempt_record(&limits);
    reserve_attempt_budget(&mut record, &reservation)?;
    record.pending_attempt = Some(PendingAttemptQuota {
        receipt: AttemptQuotaReceipt {
            reservation: reservation.clone(),
            phase: AttemptQuotaPhase::Held,
        },
    });
    store.kv.create(&key, serde_json::to_vec(&record)?.into()).await?;
    store.reserve_attempt(&reservation, &limits).await?;
    let entry = store.kv.entry(&key).await?.ok_or("not found")?;
    let mut recovered: QuotaRecord = serde_json::from_slice(&entry.value)?;
    assert_eq!(
        (
            recovered.cpu_seconds_used,
            recovered.wall_seconds_used,
            recovered.active_leases
        ),
        (3, 7, 1)
    );
    assert!(recovered.pending_attempt.is_none());
    release_attempt_budget(&mut recovered, &reservation)?;
    recovered.pending_attempt = Some(PendingAttemptQuota {
        receipt: AttemptQuotaReceipt {
            reservation: reservation.clone(),
            phase: AttemptQuotaPhase::Released,
        },
    });
    store
        .kv
        .update(&key, serde_json::to_vec(&recovered)?.into(), entry.revision)
        .await?;
    assert!(!store.release_attempt(&reservation, &limits).await?);
    let record = store.read_record(&reservation.tenant).await?.ok_or("not found")?;
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (0, 0, 0)
    );
    assert!(record.pending_attempt.is_none());
    Ok(())
}
#[tokio::test]
async fn nats_attempt_receipts_keep_the_budget_row_bounded_and_block_replays() -> Result<(), Box<dyn std::error::Error>>
{
    let (_server, store) = local_attempt_store().await?;
    let limits = attempt_limits();
    for index in 0..128 {
        let reservation = reservation(&format!("small-{index}"))?;
        store.reserve_attempt(&reservation, &limits).await?;
        store.reserve_attempt(&reservation, &limits).await?;
        assert!(store.release_attempt(&reservation, &limits).await?);
        assert!(!store.release_attempt(&reservation, &limits).await?);
        assert!(store.reserve_attempt(&reservation, &limits).await.is_err());
    }
    let reservation = reservation("cancelled-before-admission")?;
    assert!(!store.release_attempt(&reservation, &limits).await?);
    assert!(store.reserve_attempt(&reservation, &limits).await.is_err());
    let entry = store
        .kv
        .entry(nats_kv_keys(&reservation.tenant).0)
        .await?
        .ok_or("not found")?;
    assert!(
        entry.value.len() < 4096,
        "attempt history must not grow the monthly budget row"
    );
    let record: QuotaRecord = serde_json::from_slice(&entry.value)?;
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (0, 0, 0)
    );
    Ok(())
}
#[tokio::test]
async fn nats_concurrent_duplicate_attempts_charge_and_refund_once() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let store = std::sync::Arc::new(store);
    let reservation = reservation("concurrent-once")?;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let store = store.clone();
        let reservation = reservation.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            store.reserve_attempt(&reservation, &attempt_limits()).await
        }));
    }
    for task in tasks {
        task.await??;
    }
    let record = store.read_record(&reservation.tenant).await?.ok_or("not found")?;
    assert_eq!(
        (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases),
        (3, 7, 1)
    );
    assert!(store.release_attempt(&reservation, &attempt_limits()).await?);
    assert!(!store.release_attempt(&reservation, &attempt_limits()).await?);
    Ok(())
}
fn tenant(s: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(s)?)
}

#[tokio::test]
async fn first_request_within_cap_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let result = store.check_and_reserve(&tenant("acme")?, 100, 300, 3600, 7200).await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn request_exceeding_cpu_cap_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    store.check_and_reserve(&tenant("acme")?, 3500, 300, 3600, 7200).await?;
    let result = store.check_and_reserve(&tenant("acme")?, 200, 300, 3600, 7200).await;
    assert!(matches!(result, Err(LeaseError::QuotaExceeded { .. })));
    Ok(())
}

#[tokio::test]
async fn request_exceeding_wall_cap_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    store.check_and_reserve(&tenant("acme")?, 100, 7000, 3600, 7200).await?;
    let result = store.check_and_reserve(&tenant("acme")?, 100, 300, 3600, 7200).await;
    assert!(matches!(result, Err(LeaseError::QuotaExceeded { .. })));
    Ok(())
}

#[tokio::test]
async fn release_restores_capacity() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    store.check_and_reserve(&tenant("acme")?, 3500, 300, 3600, 7200).await?;
    store.release(&tenant("acme")?, 3500, 300).await?;
    let result = store.check_and_reserve(&tenant("acme")?, 200, 300, 3600, 7200).await;
    assert!(result.is_ok());
    Ok(())
}
#[tokio::test]
async fn different_tenants_have_independent_quotas() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    store
        .check_and_reserve(&tenant("tenant-a")?, 3500, 7000, 3600, 7200)
        .await?;
    let result = store
        .check_and_reserve(&tenant("tenant-b")?, 100, 300, 3600, 7200)
        .await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn exact_cap_is_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let result = store.check_and_reserve(&tenant("acme")?, 3600, 7200, 3600, 7200).await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn one_over_cap_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let result = store.check_and_reserve(&tenant("acme")?, 3601, 7200, 3600, 7200).await;
    assert!(matches!(result, Err(LeaseError::QuotaExceeded { .. })));
    Ok(())
}

#[tokio::test]
async fn active_lease_cap_blocks_when_at_limit() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let t = tenant("cap-tenant")?;
    store
        .seed_record(
            &t,
            QuotaRecord {
                active_leases: 2,
                ..QuotaRecord::with_snapshot_quota(3600, 7200, SnapshotQuota::new(10, 1000), 2)
            },
        )
        .await;
    let result = store.check_active_lease_cap(&t).await;
    assert!(matches!(result, Err(LeaseError::TooManyActiveLeases { .. })));
    Ok(())
}

#[tokio::test]
async fn snapshot_quota_publish_overflow_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let t = tenant("snap-tenant")?;
    let defaults = SnapshotQuota::new(1, 1000);
    store.record_snapshot_published(&t, 500, defaults.clone()).await?;
    let result = store.record_snapshot_published(&t, 100, defaults).await;
    assert!(matches!(result, Err(LeaseError::SnapshotQuotaExceeded { .. })));
    Ok(())
}

#[tokio::test]
async fn prewarm_quota_dispatch_overflow_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let t = tenant("prewarm-tenant")?;
    store
        .seed_record(
            &t,
            QuotaRecord {
                prewarm: PrewarmQuota {
                    active_count: 2,
                    count_cap: 2,
                    total_cpu_seconds: 0,
                    cpu_cap: 3600,
                },
                ..QuotaRecord::new(3600, 7200)
            },
        )
        .await;
    let result = store.check_prewarm_dispatch(&t, 60).await;
    assert!(matches!(result, Err(LeaseError::PrewarmQuotaExceeded { .. })));
    Ok(())
}

async fn make_nats_store(
    bucket: &str,
) -> Result<(crate::test_support::LocalNats, NatsKvQuotaStore), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let js = async_nats::jetstream::new(server.client.clone());
    let store = NatsKvQuotaStore::new(js, bucket).await?;
    Ok((server, store))
}

#[tokio::test]
async fn nats_kv_first_request_within_cap_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let bucket = format!("sandbox-quota-test-{}", crate::generated_ulid::generated_ulid());
    let (_server, store) = make_nats_store(&bucket).await?;
    let result = store.check_and_reserve(&tenant("acme")?, 100, 300, 3600, 7200).await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn nats_kv_over_cap_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let bucket = format!("sandbox-quota-test-{}", crate::generated_ulid::generated_ulid());
    let (_server, store) = make_nats_store(&bucket).await?;
    store.check_and_reserve(&tenant("acme")?, 3500, 300, 3600, 7200).await?;
    let result = store.check_and_reserve(&tenant("acme")?, 200, 300, 3600, 7200).await;
    assert!(matches!(result, Err(LeaseError::QuotaExceeded { .. })));
    Ok(())
}

#[tokio::test]
async fn nats_kv_release_restores_capacity() -> Result<(), Box<dyn std::error::Error>> {
    let bucket = format!("sandbox-quota-test-{}", crate::generated_ulid::generated_ulid());
    let (_server, store) = make_nats_store(&bucket).await?;
    store.check_and_reserve(&tenant("acme")?, 3500, 300, 3600, 7200).await?;
    store.release(&tenant("acme")?, 3500, 300).await?;
    let result = store.check_and_reserve(&tenant("acme")?, 200, 300, 3600, 7200).await;
    assert!(result.is_ok());
    Ok(())
}
#[tokio::test]
async fn nats_kv_concurrent_reserves_do_not_double_spend() -> Result<(), Box<dyn std::error::Error>> {
    let bucket = format!("sandbox-quota-test-{}", crate::generated_ulid::generated_ulid());
    let (_server, store) = make_nats_store(&bucket).await?;
    let store = std::sync::Arc::new(store);

    let half_cpu: u32 = 1800;
    let half_wall: u32 = 3600;
    let cap_cpu: u32 = 3600;
    let cap_wall: u32 = 7200;

    let t = tenant("concurrent-tenant")?;

    let s1 = std::sync::Arc::clone(&store);
    let t1 = t.clone();
    let task1 = tokio::spawn(async move { s1.check_and_reserve(&t1, half_cpu, half_wall, cap_cpu, cap_wall).await });

    let s2 = std::sync::Arc::clone(&store);
    let t2 = t.clone();
    let task2 = tokio::spawn(async move { s2.check_and_reserve(&t2, half_cpu, half_wall, cap_cpu, cap_wall).await });

    let r1 = task1.await?;
    let r2 = task2.await?;

    assert!(r1.is_ok(), "task1 failed: {r1:?}");
    assert!(r2.is_ok(), "task2 failed: {r2:?}");

    let key = nats_kv_keys(&t).0;
    let entry = store.kv.entry(&key).await?.ok_or("not found")?;
    let record: QuotaRecord = serde_json::from_slice(&entry.value)?;

    assert_eq!(
        record.cpu_seconds_used, cap_cpu,
        "cpu spend must equal sum of both tasks"
    );
    assert_eq!(
        record.wall_seconds_used, cap_wall,
        "wall spend must equal sum of both tasks"
    );
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod typed_errors;
