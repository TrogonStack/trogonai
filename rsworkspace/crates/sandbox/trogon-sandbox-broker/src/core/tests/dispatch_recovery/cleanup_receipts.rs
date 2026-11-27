use super::*;

#[tokio::test]
async fn native_ambiguous_dispatch_timeout_keeps_an_actionable_pending_lease() -> TestResult {
    assert_native_dispatch_remains_recoverable(None).await?;
    Ok(())
}

#[tokio::test]
async fn native_ambiguous_dispatch_reply_keeps_an_actionable_pending_lease() -> TestResult {
    assert_native_dispatch_remains_recoverable(Some(Bytes::from_static(b"not a lease receipt"))).await?;
    Ok(())
}

#[tokio::test]
async fn native_dispatch_receipt_without_exact_target_identity_stays_pending() -> TestResult {
    assert_native_dispatch_remains_recoverable(Some(mock_provider_response()?)).await?;
    Ok(())
}

#[tokio::test]
async fn targeted_worker_denials_keep_claims_until_exact_cleanup_then_preserve_failed_reason() -> TestResult {
    for denial in [
        ProviderDispatchReply::LaunchFailed {
            provider_error: "image unavailable".into(),
        },
        ProviderDispatchReply::DispatchRejected {
            decode_error: "invalid dispatch".into(),
        },
    ] {
        let expected = match &denial {
            ProviderDispatchReply::LaunchFailed { provider_error } => LeaseError::ProviderLaunchFailed {
                provider: ProviderKind::Fly,
                reason: provider_error.clone(),
            },
            ProviderDispatchReply::DispatchRejected { decode_error } => LeaseError::ProviderRejectedDispatch {
                provider: ProviderKind::Fly,
                reason: decode_error.clone(),
            },
            _ => return Err("unexpected denial variant".into()),
        };
        let (core, bus, drive, request) = native_failure_fixture(Some(serde_json::to_vec(&denial)?.into())).await?;
        let response = core.handle_lease_request(request.clone()).await;
        assert_eq!(response, LeaseResponse::Denied(expected));
        let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
        assert_eq!(record.status, LeaseStatus::Cancelling);
        assert_eq!(
            core.drive_store
                .get(&drive)
                .await?
                .ok_or("record missing")?
                .attached_lease,
            Some(record.lease_id.clone())
        );
        assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 1);
        assert!(
            core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Failed)
                .await
                .is_err()
        );
        assert_eq!(
            core.drive_store
                .get(&drive)
                .await?
                .ok_or("record missing")?
                .attached_lease,
            Some(record.lease_id.clone())
        );
        assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 1);
        bus.cleanup_mode.store(1, Ordering::SeqCst);
        core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Failed)
            .await?;
        assert!(
            core.drive_store
                .get(&drive)
                .await?
                .ok_or("record missing")?
                .attached_lease
                .is_none()
        );
        assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 0);
        assert_eq!(
            bus.last_release
                .lock()
                .map_err(|_| "poisoned lock")?
                .as_ref()
                .ok_or("missing value")?
                .reason,
            LeaseReleasedReason::Failed
        );
    }
    Ok(())
}

#[tokio::test]
async fn targeted_worker_denial_releases_its_claims_once_the_worker_confirms_cleanup() -> TestResult {
    let denial = ProviderDispatchReply::LaunchFailed {
        provider_error: "insufficient free disk space".into(),
    };
    let (core, bus, drive, request) = native_failure_fixture(Some(serde_json::to_vec(&denial)?.into())).await?;
    bus.cleanup_mode.store(1, Ordering::SeqCst);
    let response = core.handle_lease_request(request.clone()).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::ProviderLaunchFailed { .. })),
        "{response:?}"
    );
    let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    assert_eq!(record.status, LeaseStatus::Released);
    assert!(
        core.drive_store
            .get(&drive)
            .await?
            .ok_or("record missing")?
            .attached_lease
            .is_none()
    );
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 0);
    assert_eq!(
        bus.last_release
            .lock()
            .map_err(|_| "poisoned lock")?
            .as_ref()
            .ok_or("missing value")?
            .reason,
        LeaseReleasedReason::Failed
    );
    Ok(())
}

#[tokio::test]
async fn targeted_release_retries_keep_the_first_durable_release_time() -> TestResult {
    let (core, bus, _, request) = native_failure_fixture(None).await?;
    assert!(matches!(
        core.handle_lease_request(request).await,
        LeaseResponse::Denied(_)
    ));
    let original = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    let before = SystemTime::now();
    assert!(
        core.handle_cancel(original.lease_id.clone(), LeaseReleasedReason::Completed)
            .await
            .is_err()
    );
    let journal = core
        .idempotency
        .get_attempt(
            &original.tenant,
            &original
                .dispatch_attempt
                .as_ref()
                .ok_or("missing value")?
                .idempotency_key,
        )
        .await?
        .ok_or("record missing")?;
    let journal = serde_json::to_value(journal)?;
    let first_time = journal["dispatch_attempt"]["cleanup_started_at"].clone();
    let stamped: SystemTime = serde_json::from_value(first_time.clone()).map_err(|error| {
        format!("cleanup must durably stamp its first release time before requesting teardown: {error}")
    })?;
    assert!(stamped >= before && stamped <= SystemTime::now());

    core.active_leases.put(&original).await?;
    bus.cleanup_mode.store(1, Ordering::SeqCst);
    core.reconcile_expired_leases().await;
    let published = serde_json::to_value(
        bus.last_release
            .lock()
            .map_err(|_| "poisoned lock")?
            .as_ref()
            .ok_or("missing value")?,
    )?;
    assert_eq!(published["released_at"], first_time);
    Ok(())
}

#[tokio::test]
async fn targeted_cleanup_preserves_the_first_reason_after_concurrent_retries() -> TestResult {
    for first_reason in [LeaseReleasedReason::Failed, LeaseReleasedReason::Cancelled] {
        let (core, bus, drive, request) = native_failure_fixture(None).await?;
        assert!(matches!(
            core.handle_lease_request(request.clone()).await,
            LeaseResponse::Denied(_)
        ));
        let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
        assert!(core.handle_cancel(record.lease_id.clone(), first_reason).await.is_err());
        let (cancel, failed) = tokio::join!(
            core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled),
            core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Failed),
        );
        assert!(cancel.is_err());
        assert!(failed.is_err());
        assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 1);
        assert_eq!(
            core.drive_store
                .get(&drive)
                .await?
                .ok_or("record missing")?
                .attached_lease,
            Some(record.lease_id.clone())
        );
        bus.cleanup_mode.store(1, Ordering::SeqCst);
        core.reconcile_expired_leases().await;
        assert_eq!(
            bus.last_release
                .lock()
                .map_err(|_| "poisoned lock")?
                .as_ref()
                .ok_or("missing value")?
                .reason,
            first_reason
        );
        assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 0);
        assert!(
            core.drive_store
                .get(&drive)
                .await?
                .ok_or("record missing")?
                .attached_lease
                .is_none()
        );
    }
    Ok(())
}

#[tokio::test]
async fn cleanup_reason_journal_recovers_when_the_active_index_did_not_commit() -> TestResult {
    let (core, bus, drive, request) = native_failure_fixture(None).await?;
    assert!(matches!(
        core.handle_lease_request(request.clone()).await,
        LeaseResponse::Denied(_)
    ));
    let stale = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    assert!(
        core.handle_cancel(stale.lease_id.clone(), LeaseReleasedReason::Failed)
            .await
            .is_err()
    );
    core.active_leases.put(&stale).await?;
    bus.cleanup_mode.store(1, Ordering::SeqCst);
    core.reconcile_expired_leases().await;
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 0);
    assert!(
        core.drive_store
            .get(&drive)
            .await?
            .ok_or("record missing")?
            .attached_lease
            .is_none()
    );
    assert_eq!(
        bus.last_release
            .lock()
            .map_err(|_| "poisoned lock")?
            .as_ref()
            .ok_or("missing value")?
            .reason,
        LeaseReleasedReason::Failed
    );
    Ok(())
}

#[tokio::test]
async fn a_late_provider_grant_cannot_revive_a_cancelled_dispatch_attempt() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(GatedReceiptBus {
        entered: Default::default(),
        resume: Default::default(),
    });
    core.bus = bus.clone();
    let core = Arc::new(core);
    let request = make_request(vec![ProviderKind::Fly])?;
    let allocating = tokio::spawn({
        let core = core.clone();
        async move { core.handle_lease_request(request).await }
    });
    tokio::time::timeout(Duration::from_secs(2), bus.entered.notified()).await?;
    let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    assert!(
        record
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.pending_dispatch.is_some()),
        "the in-flight dispatch is journaled before the cancel"
    );
    core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;
    let cancelled = core
        .active_leases
        .get(&record.lease_id)
        .await?
        .ok_or("record missing")?;
    assert!(
        cancelled
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.pending_dispatch.is_none()),
        "a cancelled attempt drops its journaled dispatch"
    );
    bus.resume.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(2), allocating).await??;
    assert!(matches!(response, LeaseResponse::Denied(_)), "{response:?}");
    assert_eq!(
        core.active_leases
            .get(&record.lease_id)
            .await?
            .ok_or("record missing")?
            .status,
        LeaseStatus::Released
    );
    assert_eq!(core.quota.active_lease_count(&record.tenant).await?, 0);
    Ok(())
}

#[tokio::test]
async fn targeted_cleanup_requires_receipt_and_preserves_exact_attempt_accounting() -> TestResult {
    let (core, bus, drive, request) = native_failure_fixture(None).await?;
    let _ = core.handle_lease_request(request.clone()).await;
    let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    assert!(
        core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
            .await
            .is_err()
    );
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 1);
    assert_eq!(
        core.drive_store
            .get(&drive)
            .await?
            .ok_or("record missing")?
            .attached_lease,
        Some(record.lease_id.clone())
    );
    bus.cleanup_mode.store(1, Ordering::SeqCst);
    core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;
    core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 0);
    assert!(
        core.drive_store
            .get(&drive)
            .await?
            .ok_or("record missing")?
            .attached_lease
            .is_none()
    );
    assert_eq!(
        core.active_leases
            .get(&record.lease_id)
            .await?
            .ok_or("record missing")?
            .status,
        LeaseStatus::Released
    );
    assert!(matches!(
        core.handle_lease_request(request).await,
        LeaseResponse::Denied(_)
    ));
    assert_eq!(bus.dispatches.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn failed_release_notification_remains_reconcilable_without_double_refund() -> TestResult {
    let (core, bus, _, request) = native_failure_fixture(None).await?;
    let tenant = request.tenant.clone();
    let _ = core.handle_lease_request(request).await;
    let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    bus.cleanup_mode.store(1, Ordering::SeqCst);
    bus.fail_release_publish.store(true, Ordering::SeqCst);
    assert!(
        core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
            .await
            .is_err()
    );
    assert_eq!(
        core.active_leases
            .get(&record.lease_id)
            .await?
            .ok_or("record missing")?
            .status,
        LeaseStatus::Cancelling
    );
    assert_eq!(core.quota.active_lease_count(&tenant).await?, 0);
    bus.fail_release_publish.store(false, Ordering::SeqCst);
    core.reconcile_expired_leases().await;
    assert_eq!(
        core.active_leases
            .get(&record.lease_id)
            .await?
            .ok_or("record missing")?
            .status,
        LeaseStatus::Released
    );
    assert_eq!(bus.released_events.load(Ordering::SeqCst), 1);
    assert_eq!(core.quota.active_lease_count(&tenant).await?, 0);
    Ok(())
}

#[tokio::test]
async fn targeted_quota_refusal_preserves_the_public_error_without_dispatching() -> TestResult {
    let (mut core, bus, drive, request) = native_failure_fixture(None).await?;
    core.config.default_cpu_cap = 1;
    let response = core.handle_lease_request(request.clone()).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::QuotaExceeded { .. })),
        "{response:?}"
    );
    assert_eq!(bus.dispatches.load(Ordering::SeqCst), 0);
    assert!(
        core.drive_store
            .get(&drive)
            .await?
            .ok_or("record missing")?
            .attached_lease
            .is_none()
    );
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 0);
    Ok(())
}

#[tokio::test]
async fn purge_cannot_accept_a_release_only_cleanup_receipt() -> TestResult {
    let (core, bus, drive, request) = native_failure_fixture(None).await?;
    let _ = core.handle_lease_request(request.clone()).await;
    let record = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    bus.cleanup_mode.store(2, Ordering::SeqCst);
    assert!(
        core.cancel_lease(record.lease_id.clone(), CancelIntent::Purge)
            .await
            .is_err()
    );
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 1);
    assert_eq!(
        core.drive_store
            .get(&drive)
            .await?
            .ok_or("record missing")?
            .attached_lease,
        Some(record.lease_id)
    );
    Ok(())
}
