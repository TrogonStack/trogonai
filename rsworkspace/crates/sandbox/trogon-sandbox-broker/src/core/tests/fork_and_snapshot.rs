use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn fork_retry_with_same_key_replays_without_a_second_checkpoint() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let source_lease_id = LeaseId::new("lease-source");
    let bus = Arc::new(CountingForkBus {
        checkpoints: std::sync::atomic::AtomicUsize::new(0),
    });
    let core = make_core_with_bus(bus.clone())?;
    core.active_leases
        .put(&running_source_record(&tenant, &source_lease_id)?)
        .await?;

    let first = core
        .handle_fork_request(make_fork_request(&tenant, &source_lease_id))
        .await;
    let second = core
        .handle_fork_request(make_fork_request(&tenant, &source_lease_id))
        .await;

    let (LeaseResponse::Granted(first), LeaseResponse::Granted(second)) = (first, second) else {
        return Err("both forks should be granted".into());
    };
    assert_eq!(first.lease_id, second.lease_id);
    assert_eq!(
        bus.checkpoints.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "retry must not checkpoint again"
    );
    Ok(())
}

#[tokio::test]
async fn fork_denied_when_source_lease_not_found() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;

    let response = core
        .handle_fork_request(make_fork_request(&tenant, &LeaseId::new("missing")))
        .await;

    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::SourceLeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn fork_denied_when_source_lease_not_running() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let source_lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    let mut record = running_source_record(&tenant, &source_lease_id)?;
    record.status = LeaseStatus::Released;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_fork_request(make_fork_request(&tenant, &source_lease_id))
        .await;

    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::SourceLeaseNotRunning { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn snapshot_lease_returns_a_receipt_that_seeds_a_lease() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let response = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            accept_native: false.into(),
            tenant: tenant.clone(),
            lease_id,
        })
        .await;

    let SnapshotLeaseResponse::Created { receipt } = response else {
        return Err(format!("expected a receipt, got {response:?}").into());
    };
    assert_eq!(receipt.reference.repository, "snapshots/test-tenant");
    Ok(())
}

#[tokio::test]
async fn direct_oci_capture_records_its_actual_receipt_in_snapshot_accounting() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    let response = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            accept_native: false.into(),
            tenant: tenant.clone(),
            lease_id,
        })
        .await;
    let SnapshotLeaseResponse::Created { receipt } = response else {
        return Err(format!("expected a created OCI capture, got {response:?}").into());
    };
    let budget = core.quota.quota_record(&tenant).await?.ok_or("missing record")?;
    assert_eq!(
        (budget.snapshot.count_used, budget.snapshot.bytes_used),
        (1, receipt.bytes),
        "direct captures must account for the worker's actual receipt bytes"
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_lease_hides_foreign_leases_and_surfaces_failures() -> TestResult {
    let owner = TenantId::new("owner-tenant")?;
    let lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_err_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    core.active_leases
        .put(&running_source_record(&owner, &lease_id)?)
        .await?;

    let foreign = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            accept_native: false.into(),
            tenant: TenantId::new("other-tenant")?,
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(matches!(
        foreign,
        SnapshotLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));

    let failed = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            accept_native: false.into(),
            tenant: owner,
            lease_id,
        })
        .await;
    assert!(matches!(
        failed,
        SnapshotLeaseResponse::Denied(LeaseError::CheckpointRunningFailed { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn snapshot_and_fork_are_denied_when_the_lease_turned_snapshots_off() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    core.active_leases
        .put(&unretained_source_record(&tenant, &lease_id, snapshots_off())?)
        .await?;

    let snapshot = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            accept_native: false.into(),
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(
        matches!(
            snapshot,
            SnapshotLeaseResponse::Denied(LeaseError::SnapshotsDisabled { .. })
        ),
        "{snapshot:?}"
    );

    let fork = core.handle_fork_request(make_fork_request(&tenant, &lease_id)).await;
    assert!(
        matches!(fork, LeaseResponse::Denied(LeaseError::SnapshotsDisabled { .. })),
        "{fork:?}"
    );
    Ok(())
}

#[tokio::test]
async fn releasing_an_unmonitored_lease_purges_its_command_history() -> TestResult {
    let kept = release_with_logs(trogon_sandbox_contract::RetentionToggles {
        monitoring: false.into(),
        ..Default::default()
    })
    .await?;
    assert!(!kept);
    Ok(())
}

#[tokio::test]
async fn releasing_a_monitored_lease_keeps_its_command_history() -> TestResult {
    assert!(release_with_logs(Default::default()).await?);
    Ok(())
}

#[tokio::test]
async fn snapshot_delete_reports_unsupported_deleted_and_not_found() -> TestResult {
    let digest: Digest = format!("sha256:{}", "b".repeat(64)).parse()?;
    let tenant = TenantId::new("test-tenant")?;
    let req = || SnapshotDeleteRequest {
        tenant: tenant.clone(),
        digest: digest.clone(),
    };
    let checkpoint_response = checkpoint_ok_response()?;
    let dispatch_response = mock_provider_response()?;
    let bus = || {
        Arc::new(RoutingBus {
            checkpoint_response: checkpoint_response.clone(),
            dispatch_response: dispatch_response.clone(),
        })
    };

    let bare = make_core_with_bus(bus())?;
    assert_eq!(
        bare.handle_snapshot_delete(req()).await,
        SnapshotDeleteResponse::Unsupported
    );

    let removed = make_core_with_bus(bus())?.with_snapshot_remover(Arc::new(FixedRemover(
        crate::snapshot_tracker::SnapshotRemoval::Removed {
            bytes: 9,
            generation: None,
        },
    )));
    assert_eq!(
        removed.handle_snapshot_delete(req()).await,
        SnapshotDeleteResponse::Deleted { bytes: 9 }
    );

    let missing = make_core_with_bus(bus())?.with_snapshot_remover(Arc::new(FixedRemover(
        crate::snapshot_tracker::SnapshotRemoval::NotFound,
    )));
    assert_eq!(
        missing.handle_snapshot_delete(req()).await,
        SnapshotDeleteResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn fork_denied_when_checkpoint_fails() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let source_lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_err_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    let record = running_source_record(&tenant, &source_lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_fork_request(make_fork_request(&tenant, &source_lease_id))
        .await;

    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::CheckpointRunningFailed { .. })
    ));

    let source_still_running = core
        .active_leases
        .get(&source_lease_id)
        .await?
        .ok_or("missing record")?;
    assert_eq!(source_still_running.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn fork_denied_when_tenant_mismatches_source() -> TestResult {
    let source_lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    let record = running_source_record(&TenantId::new("owner-tenant")?, &source_lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_fork_request(make_fork_request(&TenantId::new("other-tenant")?, &source_lease_id))
        .await;

    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::SourceLeaseNotFound { .. })
    ));
    Ok(())
}
