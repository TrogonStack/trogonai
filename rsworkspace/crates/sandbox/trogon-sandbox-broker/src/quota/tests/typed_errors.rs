use std::io;

use async_nats::jetstream::kv;
use trogon_sandbox_contract::SnapshotAccountingRefusal;

use super::*;

fn seeded_record(cpu: u32, wall: u32, active: u32, reservation: &AttemptQuotaReservation) -> QuotaRecord {
    let mut record = new_attempt_record(&attempt_limits());
    record.cpu_seconds_used = cpu;
    record.wall_seconds_used = wall;
    record.active_leases = active;
    record.prewarm.total_cpu_seconds = u64::from(reservation.cpu_seconds);
    record
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the test asserts the operator-facing message text itself"
    )
)]
fn store_errors_describe_the_failure() -> Result<(), Box<dyn std::error::Error>> {
    let serde_error = serde_json::from_str::<QuotaRecord>("not json").err().ok_or("parsed")?;
    let cases: Vec<(QuotaStoreError, &str)> = vec![
        (
            QuotaStoreError::AttemptAccountingUnavailable,
            "atomic dispatch accounting is unavailable",
        ),
        (
            QuotaStoreError::AttemptIdentityReused,
            "quota attempt identity was reused",
        ),
        (QuotaStoreError::HeldRecordMissing, "held quota record is missing"),
        (
            QuotaStoreError::AttemptCpuInconsistent,
            "attempt CPU accounting is inconsistent",
        ),
        (
            QuotaStoreError::AttemptWallInconsistent,
            "attempt wall accounting is inconsistent",
        ),
        (
            QuotaStoreError::AttemptActiveInconsistent,
            "attempt active accounting is inconsistent",
        ),
        (
            QuotaStoreError::AttemptPrewarmCpuInconsistent,
            "attempt prewarm CPU accounting is inconsistent",
        ),
        (
            QuotaStoreError::AttemptPrewarmCountInconsistent,
            "attempt prewarm count is inconsistent",
        ),
        (
            QuotaStoreError::BudgetDisappeared,
            "quota budget disappeared during settlement",
        ),
        (
            QuotaStoreError::ReceiptDisappeared,
            "quota receipt disappeared during settlement",
        ),
        (
            QuotaStoreError::ReceiptAdvanced,
            "quota receipt advanced while the budget mutation remained pending",
        ),
        (
            QuotaStoreError::CasRetriesExhausted,
            "quota store CAS failed after max retries",
        ),
        (
            QuotaStoreError::ReleaseCasRetriesExhausted,
            "quota store CAS failed after max retries on release",
        ),
        (
            QuotaStoreError::ReceiptCasRetriesExhausted,
            "quota receipt CAS retries exhausted",
        ),
        (
            QuotaStoreError::SettlementCasRetriesExhausted,
            "quota settlement CAS retries exhausted",
        ),
        (
            QuotaStoreError::LegacyInventoryTooLarge,
            "legacy snapshot inventory exceeds reconciliation bound",
        ),
        (
            QuotaStoreError::LegacyUsageRequiresReconciliation,
            "legacy snapshot usage requires reconciliation",
        ),
        (
            QuotaStoreError::LegacyObservation(SnapshotAccountingRefusal::Backend),
            "snapshot legacy observation: Backend",
        ),
        (
            QuotaStoreError::LegacyQuarantine(SnapshotAccountingRefusal::Backend),
            "snapshot legacy quarantine: Backend",
        ),
        (
            QuotaStoreError::SnapshotAccounting(SnapshotAccountingRefusal::Backend),
            "snapshot accounting: Backend",
        ),
        (
            QuotaStoreError::snapshot_store(io::Error::other("gone")),
            "snapshot store: gone",
        ),
        (
            QuotaStoreError::from(LeaseError::Backend {
                message: "lease backend".to_string(),
            }),
            "lease backend",
        ),
    ];
    for (error, expected) in cases {
        assert!(
            error.to_string().contains(expected),
            "{error} should mention {expected}"
        );
    }
    let wrapped = [
        QuotaStoreError::from(serde_error),
        QuotaStoreError::from(kv::EntryError::new(kv::EntryErrorKind::Other)),
        QuotaStoreError::from(kv::CreateError::new(kv::CreateErrorKind::Other)),
        QuotaStoreError::from(kv::UpdateError::new(kv::UpdateErrorKind::Other)),
        QuotaStoreError::from(kv::HistoryError::new(kv::HistoryErrorKind::Other)),
        QuotaStoreError::from(kv::WatcherError::new(kv::WatcherErrorKind::Other)),
        QuotaStoreError::from(jetstream::context::CreateKeyValueError::new(
            jetstream::context::CreateKeyValueErrorKind::TimedOut,
        )),
    ];
    let prefixes = [
        "quota record serialization",
        "quota entry read",
        "quota entry create",
        "quota entry update",
        "quota key listing",
        "quota key stream",
        "quota bucket",
    ];
    for (error, prefix) in wrapped.iter().zip(prefixes) {
        assert!(
            error.to_string().starts_with(prefix),
            "{error} should start with {prefix}"
        );
    }
    Ok(())
}

#[test]
fn releasing_more_than_was_held_names_the_inconsistent_counter() -> Result<(), Box<dyn std::error::Error>> {
    let tenant_reservation = reservation("inconsistent")?;
    let mut record = seeded_record(0, 0, 0, &tenant_reservation);
    assert!(matches!(
        release_attempt_budget(&mut record, &tenant_reservation),
        Err(QuotaStoreError::AttemptCpuInconsistent)
    ));
    record.cpu_seconds_used = tenant_reservation.cpu_seconds;
    assert!(matches!(
        release_attempt_budget(&mut record, &tenant_reservation),
        Err(QuotaStoreError::AttemptWallInconsistent)
    ));
    record.wall_seconds_used = tenant_reservation.wall_seconds;
    assert!(matches!(
        release_attempt_budget(&mut record, &tenant_reservation),
        Err(QuotaStoreError::AttemptActiveInconsistent)
    ));

    let mut prewarm_reservation = reservation("inconsistent-prewarm")?;
    prewarm_reservation.accounting = LeaseAccounting::Prewarm;
    let mut record = seeded_record(0, 0, 0, &prewarm_reservation);
    record.prewarm.total_cpu_seconds = 0;
    assert!(matches!(
        release_attempt_budget(&mut record, &prewarm_reservation),
        Err(QuotaStoreError::AttemptPrewarmCpuInconsistent)
    ));
    record.prewarm.total_cpu_seconds = u64::from(prewarm_reservation.cpu_seconds);
    record.prewarm.active_count = 0;
    assert!(matches!(
        release_attempt_budget(&mut record, &prewarm_reservation),
        Err(QuotaStoreError::AttemptPrewarmCountInconsistent)
    ));
    Ok(())
}

#[tokio::test]
async fn in_memory_attempts_report_typed_identity_and_record_errors() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let limits = attempt_limits();
    let held = reservation("typed-held")?;
    store.reserve_attempt(&held, &limits).await?;
    let mut wrong = held.clone();
    wrong.cpu_seconds += 1;
    assert!(matches!(
        store.release_attempt(&wrong, &limits).await,
        Err(QuotaStoreError::AttemptIdentityReused)
    ));

    let orphan = reservation("typed-orphan")?;
    store.attempts.lock().await.insert(
        orphan.attempt_id.clone(),
        AttemptQuotaReceipt {
            reservation: orphan.clone(),
            phase: AttemptQuotaPhase::Held,
        },
    );
    store.records.lock().await.remove(&attempt_month_key(&orphan)?);
    assert!(matches!(
        store.release_attempt(&orphan, &limits).await,
        Err(QuotaStoreError::HeldRecordMissing)
    ));
    Ok(())
}

#[tokio::test]
async fn in_memory_quota_record_refuses_unreconciled_legacy_usage() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryQuotaStore::new();
    let acme = tenant("typed-legacy")?;
    let mut old = QuotaRecord::new(3600, 7200);
    old.snapshot.count_used = 1;
    store
        .records
        .lock()
        .await
        .insert(format!("{}:2020-01", acme.as_str()), old);
    assert!(matches!(
        store.quota_record(&acme).await,
        Err(QuotaStoreError::LegacyUsageRequiresReconciliation)
    ));
    Ok(())
}

#[tokio::test]
async fn the_default_store_has_no_atomic_dispatch_accounting() -> Result<(), Box<dyn std::error::Error>> {
    struct Bare;

    #[async_trait::async_trait]
    impl QuotaStore for Bare {
        async fn check_and_reserve(&self, _: &TenantId, _: u32, _: u32, _: u32, _: u32) -> Result<(), LeaseError> {
            Ok(())
        }
        async fn release(&self, _: &TenantId, _: u32, _: u32) -> Result<(), QuotaStoreError> {
            Ok(())
        }
        async fn active_lease_count(&self, _: &TenantId) -> Result<u32, QuotaStoreError> {
            Ok(0)
        }
        async fn check_active_lease_cap(&self, _: &TenantId) -> Result<(), LeaseError> {
            Ok(())
        }
        async fn increment_active_leases(&self, _: &TenantId) -> Result<(), QuotaStoreError> {
            Ok(())
        }
        async fn decrement_active_leases(&self, _: &TenantId) -> Result<(), QuotaStoreError> {
            Ok(())
        }
        async fn record_snapshot_published(&self, _: &TenantId, _: u64, _: SnapshotQuota) -> Result<(), LeaseError> {
            Ok(())
        }
        async fn record_snapshot_deleted(&self, _: &TenantId, _: u64) -> Result<(), QuotaStoreError> {
            Ok(())
        }
        async fn check_prewarm_dispatch(&self, _: &TenantId, _: u32) -> Result<(), LeaseError> {
            Ok(())
        }
        async fn record_prewarm_dispatch(&self, _: &TenantId, _: u32) -> Result<(), QuotaStoreError> {
            Ok(())
        }
        async fn record_prewarm_release(&self, _: &TenantId, _: u32) -> Result<(), QuotaStoreError> {
            Ok(())
        }
    }

    let store = Bare;
    let reservation = reservation("bare")?;
    let limits = attempt_limits();
    assert!(store.reserve_attempt(&reservation, &limits).await.is_err());
    assert!(matches!(
        store.release_attempt(&reservation, &limits).await,
        Err(QuotaStoreError::AttemptAccountingUnavailable)
    ));
    Ok(())
}

#[tokio::test]
async fn nats_mutations_report_exhausted_retries() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let store = store.with_cas_retries(0);
    let acme = tenant("typed-cas")?;
    assert!(matches!(
        store.increment_active_leases(&acme).await,
        Err(QuotaStoreError::CasRetriesExhausted)
    ));
    assert!(matches!(
        store.release(&acme, 1, 1).await,
        Err(QuotaStoreError::ReleaseCasRetriesExhausted)
    ));
    assert!(matches!(
        store.check_and_reserve(&acme, 1, 1, 3600, 7200).await,
        Err(LeaseError::Backend { .. })
    ));
    assert!(matches!(
        store
            .record_snapshot_published(&acme, 1, SnapshotQuota::default())
            .await,
        Err(LeaseError::Backend { .. })
    ));
    let held = reservation("typed-cas-attempt")?;
    assert!(matches!(
        store.reserve_attempt(&held, &attempt_limits()).await,
        Err(LeaseError::Backend { .. })
    ));
    let receipt = AttemptQuotaReceipt {
        reservation: held,
        phase: AttemptQuotaPhase::Held,
    };
    assert!(matches!(
        store.write_attempt_receipt(&receipt).await,
        Err(QuotaStoreError::ReceiptCasRetriesExhausted)
    ));
    Ok(())
}

#[tokio::test]
async fn nats_settlement_reports_its_own_failures() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let limits = attempt_limits();
    let pending_for = |reservation: &AttemptQuotaReservation| PendingAttemptQuota {
        receipt: AttemptQuotaReceipt {
            reservation: reservation.clone(),
            phase: AttemptQuotaPhase::Held,
        },
    };

    let missing = reservation("typed-settle-missing")?;
    assert!(matches!(
        store
            .settle_attempt("typed-settle-missing", &pending_for(&missing))
            .await,
        Err(QuotaStoreError::BudgetDisappeared)
    ));

    let advanced = reservation("typed-settle-advanced")?;
    let pending = pending_for(&advanced);
    let mut record = new_attempt_record(&limits);
    reserve_attempt_budget(&mut record, &advanced)?;
    record.pending_attempt = Some(pending.clone());
    store
        .kv
        .create("typed-settle-advanced", serde_json::to_vec(&record)?.into())
        .await?;
    assert!(!store.release_attempt(&advanced, &limits).await?);
    assert!(matches!(
        store.settle_attempt("typed-settle-advanced", &pending).await,
        Err(QuotaStoreError::ReceiptAdvanced)
    ));

    let exhausted = reservation("typed-settle-exhausted")?;
    let (_other_server, other) = local_attempt_store().await?;
    let other = other.with_settlement_cas_retries(0);
    assert!(matches!(
        other
            .settle_attempt("typed-settle-exhausted", &pending_for(&exhausted))
            .await,
        Err(QuotaStoreError::SettlementCasRetriesExhausted)
    ));
    Ok(())
}

#[tokio::test]
async fn nats_legacy_scan_refuses_an_unbounded_inventory() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let store = store.with_legacy_inventory_bound(0);
    let acme = tenant("typed-scan")?;
    store
        .kv
        .create(
            &quota_kv_key_for_month(&acme, 2020, 1),
            serde_json::to_vec(&QuotaRecord::new(3600, 7200))?.into(),
        )
        .await?;
    assert!(matches!(
        store.legacy_snapshot_quota(&acme).await,
        Err(QuotaStoreError::LegacyInventoryTooLarge)
    ));
    Ok(())
}

#[tokio::test]
async fn nats_quota_record_refuses_unreconciled_legacy_usage() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = local_attempt_store().await?;
    let acme = tenant("typed-nats-legacy")?;
    let mut old = QuotaRecord::new(3600, 7200);
    old.snapshot.bytes_used = 9;
    store
        .kv
        .create(
            &quota_kv_key_for_month(&acme, 2020, 1),
            serde_json::to_vec(&old)?.into(),
        )
        .await?;
    assert!(matches!(
        store.quota_record(&acme).await,
        Err(QuotaStoreError::LegacyUsageRequiresReconciliation)
    ));
    Ok(())
}
