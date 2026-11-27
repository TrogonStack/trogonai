use super::*;
use trogon_sandbox_contract::NativeSnapshotAcceptance;

#[tokio::test]
async fn pause_parks_the_deadline_and_resume_restores_it() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "pause-roundtrip").await?;

    let PauseLeaseResponse::Paused {
        reap_at,
        operation_id: None,
    } = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
            max_paused_seconds: Some(7200),
            operation_id: None,
        })
        .await
    else {
        return Err("pause should succeed".into());
    };
    let paused = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(paused.status, LeaseStatus::Paused);
    assert_eq!(paused.deadline, reap_at);
    let parked = paused.paused_remaining_seconds.ok_or("remaining time is parked")?;
    assert!((298..=300).contains(&parked), "parked {parked}");
    assert!(reap_at > SystemTime::now() + Duration::from_secs(7000));

    // A second pause changes nothing.
    let again = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
            max_paused_seconds: Some(5),
            operation_id: None,
        })
        .await;
    assert_eq!(
        again,
        PauseLeaseResponse::Paused {
            reap_at,
            operation_id: None
        }
    );

    let ResumeLeaseResponse::Resumed {
        deadline,
        operation_id: None,
    } = core
        .handle_resume_lease(ResumeLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
            operation_id: None,
        })
        .await
    else {
        return Err("resume should succeed".into());
    };
    let resumed = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(resumed.status, LeaseStatus::Running);
    assert_eq!(resumed.paused_remaining_seconds, None);
    assert_eq!(resumed.deadline, deadline);
    let left = deadline.duration_since(SystemTime::now())?.as_secs();
    assert!((297..=300).contains(&left), "left {left}");

    // A second resume returns the running lease unchanged.
    let again = core
        .handle_resume_lease(ResumeLeaseRequest {
            tenant,
            lease_id,
            operation_id: None,
        })
        .await;
    assert_eq!(
        again,
        ResumeLeaseResponse::Resumed {
            deadline,
            operation_id: None
        }
    );
    Ok(())
}

#[tokio::test]
async fn pause_caps_the_paused_budget_at_the_ceiling() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "pause-ceiling").await?;
    let PauseLeaseResponse::Paused {
        reap_at,
        operation_id: None,
    } = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant,
            lease_id,
            max_paused_seconds: Some(u32::MAX),
            operation_id: None,
        })
        .await
    else {
        return Err("pause should succeed".into());
    };
    let budget = reap_at.duration_since(SystemTime::now())?.as_secs();
    assert!(budget <= u64::from(MAX_PAUSED_SECONDS_CEILING), "{budget}");
    Ok(())
}

#[tokio::test]
async fn pause_by_another_tenant_reads_as_not_found() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let owner = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &owner, "pause-foreign").await?;
    let response = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant: TenantId::new("intruder")?,
            lease_id: lease_id.clone(),
            max_paused_seconds: None,
            operation_id: None,
        })
        .await;
    assert!(matches!(
        response,
        PauseLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    let resume = core
        .handle_resume_lease(ResumeLeaseRequest {
            tenant: TenantId::new("intruder")?,
            lease_id: lease_id.clone(),
            operation_id: None,
        })
        .await;
    assert!(matches!(
        resume,
        ResumeLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    Ok(())
}

/// Request-tenant-gated verbs that carry a lease id must still refuse a
/// lease another tenant owns, so a valid key for tenant A cannot touch
/// tenant B's lease by guessing its id.
#[tokio::test]
async fn lease_verbs_refuse_a_lease_owned_by_another_tenant() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let owner = TenantId::new("acme")?;
    let intruder = TenantId::new("intruder")?;
    let lease_id = running_lease_in(&core, &owner, "foreign-lease").await?;

    let usage = core
        .handle_get_lease_usage(GetLeaseUsageRequest {
            tenant: intruder.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(
        matches!(usage, GetLeaseUsageResponse::Denied(LeaseError::LeaseNotFound { .. })),
        "usage: {usage:?}"
    );

    let snapshot = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            accept_native: NativeSnapshotAcceptance::from(false),
            tenant: intruder.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(
        matches!(
            snapshot,
            SnapshotLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "snapshot: {snapshot:?}"
    );

    let stop = core
        .handle_stop_lease(StopLeaseRequest {
            tenant: intruder.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(
        matches!(stop, StopLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })),
        "stop: {stop:?}"
    );

    let delete = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: intruder,
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(
        matches!(delete, DeleteLeaseResponse::Denied(LeaseError::Unauthorized { .. })),
        "delete: {delete:?}"
    );

    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(record.tenant, owner);
    Ok(())
}

#[tokio::test]
async fn pause_is_unsupported_when_the_profile_cannot_pause() -> TestResult {
    let mut core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.lifecycle.supports_pause_resume = CapabilitySupport::Unsupported;
    let mut heartbeat = worker_heartbeat("no-pause", ProviderKind::Mock, profile)?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "pause-unsupported").await?;
    let response = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            max_paused_seconds: None,
            operation_id: None,
        })
        .await;
    assert_eq!(response, PauseLeaseResponse::Unsupported);
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn pause_is_unsupported_when_the_worker_answers_unsupported() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Unsupported)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "pause-worker-unsupported").await?;
    let response = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            max_paused_seconds: None,
            operation_id: None,
        })
        .await;
    assert_eq!(response, PauseLeaseResponse::Unsupported);
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn a_paused_lease_is_not_swept_before_its_reap_time() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "pause-sweep").await?;
    // The lease would have expired in 300 seconds; a paused one outlives it.
    let PauseLeaseResponse::Paused {
        reap_at,
        operation_id: None,
    } = core
        .handle_pause_lease(PauseLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            max_paused_seconds: Some(3600),
            operation_id: None,
        })
        .await
    else {
        return Err("pause should succeed".into());
    };
    assert!(reap_at > SystemTime::now() + Duration::from_secs(3000));
    Ok(())
}

#[tokio::test]
async fn pause_and_resume_record_their_marker_before_asking_the_worker() -> TestResult {
    let (core, bus) = freeze_probe_core(SandboxFreezeResponse::Done)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "freeze-marker").await?;

    let paused = core
        .handle_pause_lease(PauseLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
            max_paused_seconds: None,
        })
        .await;
    assert!(matches!(paused, PauseLeaseResponse::Paused { .. }));
    assert!(bus.saw_marker.swap(false, Ordering::SeqCst));
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Paused);
    assert_eq!(record.freeze_in_flight, None);

    let resumed = core
        .handle_resume_lease(ResumeLeaseRequest {
            operation_id: None,
            tenant,
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(matches!(resumed, ResumeLeaseResponse::Resumed { .. }));
    assert!(bus.saw_marker.load(Ordering::SeqCst));
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(record.freeze_in_flight, None);
    Ok(())
}

#[tokio::test]
async fn a_refused_pause_leaves_no_marker() -> TestResult {
    let (core, _bus) = freeze_probe_core(SandboxFreezeResponse::Denied(LeaseError::Backend {
        message: "frozen solid".into(),
    }))?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "freeze-refused").await?;
    let response = core
        .handle_pause_lease(PauseLeaseRequest {
            operation_id: None,
            tenant,
            lease_id: lease_id.clone(),
            max_paused_seconds: None,
        })
        .await;
    assert!(matches!(response, PauseLeaseResponse::Denied(_)));
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(record.freeze_in_flight, None);
    Ok(())
}

#[tokio::test]
async fn reconciler_settles_abandoned_freezes_on_the_recorded_status() -> TestResult {
    let (core, bus) = freeze_probe_core(SandboxFreezeResponse::Done)?;
    // A pause that may have frozen the worker before the broker stopped.
    let running = lease_with_stale_freeze(&core, "stalled-pause", LeaseStatus::Running, false).await?;
    // A resume that may have thawed it.
    let paused = lease_with_stale_freeze(&core, "stalled-resume", LeaseStatus::Paused, true).await?;
    // A marker still within its request's lifetime is left alone.
    let fresh = running_lease_in(&core, &TenantId::new("acme")?, "fresh-pause").await?;
    let mut record = core.active_leases.get(&fresh).await?.ok_or("the value is missing")?;
    record.freeze_in_flight = Some(FreezeInFlight {
        action: FreezeAction::from(false),
        started_at: SystemTime::now(),
        settle_attempts: 0,
        next_settle_at: None,
    });
    core.active_leases.put(&record).await?;

    core.reconcile_stalled_freezes().await;

    let mut subjects = locked(&bus.subjects)?.clone();
    subjects.sort();
    let mut expected = vec![
        subjects::provider_resume(ProviderKind::Mock, &running),
        subjects::provider_pause(ProviderKind::Mock, &paused),
    ];
    expected.sort();
    assert_eq!(subjects, expected);
    for (id, status) in [(&running, LeaseStatus::Running), (&paused, LeaseStatus::Paused)] {
        let record = core.active_leases.get(id).await?.ok_or("the value is missing")?;
        assert_eq!(record.status, status);
        assert_eq!(record.freeze_in_flight, None);
    }
    let record = core.active_leases.get(&fresh).await?.ok_or("the value is missing")?;
    assert!(record.freeze_in_flight.is_some());
    Ok(())
}

#[tokio::test]
async fn reconciler_keeps_the_marker_while_the_worker_refuses() -> TestResult {
    let (core, _bus) = freeze_probe_core(SandboxFreezeResponse::Denied(LeaseError::Backend {
        message: "worker busy".into(),
    }))?;
    let lease_id = lease_with_stale_freeze(&core, "stalled-denied", LeaseStatus::Running, false).await?;
    core.reconcile_stalled_freezes().await;
    let record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    assert!(record.freeze_in_flight.is_some());
    Ok(())
}

#[tokio::test]
async fn reconciler_backs_off_a_worker_that_keeps_refusing() -> TestResult {
    let (core, bus) = freeze_probe_core(SandboxFreezeResponse::Denied(LeaseError::Backend {
        message: "worker busy".into(),
    }))?;
    let lease_id = lease_with_stale_freeze(&core, "stalled-backoff", LeaseStatus::Running, false).await?;

    core.reconcile_stalled_freezes().await;
    core.reconcile_stalled_freezes().await;

    assert_eq!(locked(&bus.subjects)?.len(), 1);
    let marker = core
        .active_leases
        .get(&lease_id)
        .await?
        .ok_or("the value is missing")?
        .freeze_in_flight
        .ok_or("the freeze marker is missing")?;
    assert_eq!(marker.settle_attempts, 1);
    assert!(marker.next_settle_at.is_some_and(|at| at > SystemTime::now()));
    assert_eq!(core.metrics().unsettled_freezes.get(), 0);
    Ok(())
}

#[tokio::test]
async fn reconciler_stops_asking_after_the_attempt_cap_and_counts_the_lease() -> TestResult {
    let (core, bus) = freeze_probe_core(SandboxFreezeResponse::Denied(LeaseError::Backend {
        message: "worker busy".into(),
    }))?;
    let lease_id = lease_with_stale_freeze(&core, "stalled-capped", LeaseStatus::Running, false).await?;
    let mut record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    record
        .freeze_in_flight
        .as_mut()
        .ok_or("the freeze marker is missing")?
        .settle_attempts = FREEZE_SETTLE_MAX_ATTEMPTS - 1;
    core.active_leases.put(&record).await?;

    core.reconcile_stalled_freezes().await;
    assert_eq!(core.metrics().unsettled_freezes.get(), 1);

    // Even once the backoff has passed, a capped marker is not retried.
    let mut record = core.active_leases.get(&lease_id).await?.ok_or("the value is missing")?;
    record
        .freeze_in_flight
        .as_mut()
        .ok_or("the freeze marker is missing")?
        .next_settle_at = None;
    core.active_leases.put(&record).await?;
    core.reconcile_stalled_freezes().await;

    assert_eq!(locked(&bus.subjects)?.len(), 1);
    assert_eq!(core.metrics().unsettled_freezes.get(), 1);
    let marker = core
        .active_leases
        .get(&lease_id)
        .await?
        .ok_or("the value is missing")?
        .freeze_in_flight
        .ok_or("the freeze marker is missing")?;
    assert_eq!(marker.settle_attempts, FREEZE_SETTLE_MAX_ATTEMPTS);
    Ok(())
}

#[test]
fn freeze_settle_backoff_doubles_up_to_its_cap() -> TestResult {
    assert_eq!(freeze_settle_backoff(1), Duration::from_secs(30));
    assert_eq!(freeze_settle_backoff(2), Duration::from_secs(60));
    assert_eq!(freeze_settle_backoff(3), Duration::from_secs(120));
    assert_eq!(freeze_settle_backoff(40), Duration::from_secs(3600));
    Ok(())
}
