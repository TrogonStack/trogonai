use crate::active_lease::ActiveLeaseStore;
use crate::idempotency::IdempotencyStore;

use super::resolve_unsettled_freeze::resolved;
use crate::core::FREEZE_SETTLE_MAX_ATTEMPTS;

use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::LeaseRequestKey;
use trogon_sandbox_contract::{FreezeAction, FreezeInFlight, LeaseStatus, ProviderKind, TenantId};

use super::*;
use crate::active_lease::InMemoryActiveLeaseStore;
use crate::idempotency::InMemoryIdempotencyStore;
use trogon_sandbox_contract::{LeaseId, LeaseRecord};

fn stuck(id: &str, attempts: u32) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new(id),
        tenant: TenantId::new("acme")?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(600),
        spec: crate::idle::tests::spec(60, None, None)?,
        sandbox_id: "sbx".into(),
        cpu_seconds_consumed: 0,
        wall_seconds_consumed: 0,
        correlation_id: None,
        tags: Default::default(),
        name: None,
        worker_id: None,
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: Some(FreezeInFlight {
            action: FreezeAction::Resume,
            started_at: SystemTime::now() - Duration::from_secs(3600),
            settle_attempts: attempts,
            next_settle_at: Some(SystemTime::now() + Duration::from_secs(3600)),
        }),
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

async fn resolve(
    active: &InMemoryActiveLeaseStore,
    idempotency: &InMemoryIdempotencyStore,
    id: &str,
    resolution: FreezeResolution,
    force: bool,
) -> Result<FreezeResolveOutcome, Box<dyn std::error::Error>> {
    Ok(resolve_unsettled_freeze(active, idempotency, &LeaseId::new(id), resolution, force).await?)
}

#[tokio::test]
async fn clear_drops_a_marker_the_broker_gave_up_on() -> Result<(), Box<dyn std::error::Error>> {
    let active = InMemoryActiveLeaseStore::new();
    let idempotency = InMemoryIdempotencyStore::new();
    active.put(&stuck("capped", FREEZE_SETTLE_MAX_ATTEMPTS)?).await?;

    let outcome = resolve(&active, &idempotency, "capped", FreezeResolution::Clear, false).await?;
    assert_eq!(outcome, FreezeResolveOutcome::Applied);
    let record = active.get(&LeaseId::new("capped")).await?.ok_or("missing lease")?;
    assert_eq!(record.freeze_in_flight, None);
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(
        resolve(&active, &idempotency, "capped", FreezeResolution::Clear, false).await?,
        FreezeResolveOutcome::NoMarker
    );
    Ok(())
}

#[tokio::test]
async fn retry_resets_the_attempts_so_the_reconciler_asks_again() -> Result<(), Box<dyn std::error::Error>> {
    let active = InMemoryActiveLeaseStore::new();
    let idempotency = InMemoryIdempotencyStore::new();
    active.put(&stuck("capped", FREEZE_SETTLE_MAX_ATTEMPTS)?).await?;

    let outcome = resolve(&active, &idempotency, "capped", FreezeResolution::Retry, false).await?;
    assert_eq!(outcome, FreezeResolveOutcome::Applied);
    let marker = active
        .get(&LeaseId::new("capped"))
        .await?
        .ok_or("missing lease")?
        .freeze_in_flight
        .ok_or("missing marker")?;
    assert_eq!((marker.settle_attempts, marker.next_settle_at), (0, None));
    Ok(())
}

#[tokio::test]
async fn a_marker_still_being_settled_needs_force() -> Result<(), Box<dyn std::error::Error>> {
    let active = InMemoryActiveLeaseStore::new();
    let idempotency = InMemoryIdempotencyStore::new();
    active.put(&stuck("busy", 3)?).await?;

    assert_eq!(
        resolve(&active, &idempotency, "busy", FreezeResolution::Clear, false).await?,
        FreezeResolveOutcome::NotUnsettled { settle_attempts: 3 }
    );
    assert!(
        active
            .get(&LeaseId::new("busy"))
            .await?
            .ok_or("missing lease")?
            .freeze_in_flight
            .is_some()
    );
    assert_eq!(
        resolve(&active, &idempotency, "busy", FreezeResolution::Clear, true).await?,
        FreezeResolveOutcome::Applied
    );
    assert_eq!(
        resolve(&active, &idempotency, "gone", FreezeResolution::Clear, true).await?,
        FreezeResolveOutcome::NoSuchLease
    );
    Ok(())
}
#[tokio::test]
async fn a_dispatch_journalled_lease_keeps_its_journal_equal_to_its_record() -> Result<(), Box<dyn std::error::Error>> {
    let active = InMemoryActiveLeaseStore::new();
    let idempotency = InMemoryIdempotencyStore::new();
    let mut record = stuck("journalled", FREEZE_SETTLE_MAX_ATTEMPTS)?;
    record.worker_id = Some(trogon_sandbox_contract::WorkerId::new("worker-a")?);
    record.dispatch_attempt = Some(trogon_sandbox_contract::LeaseDispatchAttempt {
        epoch: None,
        id: trogon_sandbox_contract::DispatchAttemptId::new("attempt-1")?,
        idempotency_key: LeaseRequestKey::new("key")?,
        accounting: trogon_sandbox_contract::LeaseAccounting::Tenant,
        cleanup_reason: None,
        cleanup_started_at: None,
        pending_dispatch: None,
    });
    let key = LeaseRequestKey::new("key")?;
    let mut pending = record.clone();
    pending.status = LeaseStatus::Pending;
    pending.freeze_in_flight = None;
    assert!(idempotency.begin_attempt(&record.tenant, &key, &pending).await?);
    assert!(
        idempotency
            .replace_attempt(&record.tenant, &key, &pending, &record)
            .await?
    );
    active.put(&record).await?;

    let outcome = resolve(&active, &idempotency, "journalled", FreezeResolution::Clear, false).await?;
    assert_eq!(outcome, FreezeResolveOutcome::Applied);
    let stored = active.get(&LeaseId::new("journalled")).await?.ok_or("missing lease")?;
    assert_eq!(stored.freeze_in_flight, None);
    assert_eq!(idempotency.get_attempt(&record.tenant, &key).await?, Some(stored));
    Ok(())
}
#[tokio::test]
async fn a_run_that_stopped_between_the_journal_and_the_record_finishes_on_rerun()
-> Result<(), Box<dyn std::error::Error>> {
    let active = InMemoryActiveLeaseStore::new();
    let idempotency = InMemoryIdempotencyStore::new();
    let mut record = stuck("half", FREEZE_SETTLE_MAX_ATTEMPTS)?;
    record.worker_id = Some(trogon_sandbox_contract::WorkerId::new("worker-a")?);
    record.dispatch_attempt = Some(trogon_sandbox_contract::LeaseDispatchAttempt {
        epoch: None,
        id: trogon_sandbox_contract::DispatchAttemptId::new("attempt-2")?,
        idempotency_key: LeaseRequestKey::new("half-key")?,
        accounting: trogon_sandbox_contract::LeaseAccounting::Tenant,
        cleanup_reason: None,
        cleanup_started_at: None,
        pending_dispatch: None,
    });
    let key = LeaseRequestKey::new("half-key")?;
    let mut pending = record.clone();
    pending.status = LeaseStatus::Pending;
    pending.freeze_in_flight = None;
    idempotency.begin_attempt(&record.tenant, &key, &pending).await?;
    idempotency
        .replace_attempt(&record.tenant, &key, &pending, &record)
        .await?;
    active.put(&record).await?;
    // The journal moved; the process stopped before the record did.
    let cleared = resolved(&record, FreezeResolution::Clear);
    idempotency
        .replace_attempt(&record.tenant, &key, &record, &cleared)
        .await?;

    let outcome = resolve(&active, &idempotency, "half", FreezeResolution::Clear, false).await?;
    assert_eq!(outcome, FreezeResolveOutcome::Applied);
    assert_eq!(active.get(&LeaseId::new("half")).await?, Some(cleared));
    Ok(())
}
