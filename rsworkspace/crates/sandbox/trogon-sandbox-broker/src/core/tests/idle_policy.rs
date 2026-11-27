use super::*;
use crate::idle::{IdlePauser, last_activity, next_idle_action};
use trogon_sandbox_contract::IdleAction::{self, Pause, Release};

#[tokio::test]
async fn idle_pause_goes_through_the_pause_path_and_keeps_the_idle_chain() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "idle-pause").await?;
    let mut record = core.active_leases.get(&lease_id).await?.ok_or("lease record missing")?;
    let policy = idle_policy(&[(60, IdleAction::Pause), (600, IdleAction::Release)])?;
    record.spec = with_spec_parts(record.spec, |parts| parts.idle_policy = Some(policy))?;
    core.active_leases.put(&record).await?;
    let run_deadline = record.deadline;

    assert!(core.pause_idle(&record).await);

    let paused = core.active_leases.get(&lease_id).await?.ok_or("lease record missing")?;
    assert_eq!(paused.status, LeaseStatus::Paused);
    assert!(paused.auto_paused_at.is_some());
    assert!(paused.paused_remaining_seconds.is_some());
    assert_ne!(paused.deadline, run_deadline, "paused budget replaces the run deadline");
    // The Release stage still governs it, measured from the last activity.
    let quiet = last_activity(&paused);
    assert_eq!(next_idle_action(&paused, quiet + Duration::from_secs(300), true), None);
    assert_eq!(
        next_idle_action(&paused, quiet + Duration::from_secs(601), true),
        Some(IdleAction::Release)
    );

    // Resume ends the auto pause and restarts the idle clock.
    let ResumeLeaseResponse::Resumed { .. } = core
        .handle_resume_lease(ResumeLeaseRequest {
            operation_id: None,
            tenant,
            lease_id: lease_id.clone(),
        })
        .await
    else {
        return Err("resume should succeed".into());
    };
    let resumed = core.active_leases.get(&lease_id).await?.ok_or("lease record missing")?;
    assert_eq!(resumed.status, LeaseStatus::Running);
    assert_eq!(resumed.auto_paused_at, None);
    assert_eq!(next_idle_action(&resumed, SystemTime::now(), true), None);
    Ok(())
}

#[tokio::test]
async fn a_refused_idle_pause_keeps_its_backoff_across_a_restart() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Denied(LeaseError::Backend {
        message: "worker busy".into(),
    }))?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "idle-refused").await?;
    let record = core.active_leases.get(&lease_id).await?.ok_or("lease record missing")?;

    assert!(!core.pause_idle(&record).await);
    let backed_off = core.active_leases.get(&lease_id).await?.ok_or("lease record missing")?;
    assert_eq!(backed_off.status, LeaseStatus::Running);
    let retry = backed_off
        .idle_pause_retry
        .ok_or("the refusal is recorded on the lease")?;
    assert_eq!(retry.failures, 1);
    assert!(retry.retry_at > SystemTime::now());

    // A restarted broker reads the backoff from the record and does not
    // ask the provider again before it is due.
    let mut restarted = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    restarted.active_leases = Arc::clone(&core.active_leases);
    assert!(!restarted.pause_idle(&backed_off).await);
    let held = restarted
        .active_leases
        .get(&lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert_eq!(held.status, LeaseStatus::Running);

    let mut due = held.clone();
    due.idle_pause_retry = Some(trogon_sandbox_contract::IdlePauseRetry {
        failures: 1,
        retry_at: SystemTime::now() - Duration::from_secs(1),
    });
    restarted.active_leases.put(&due).await?;
    assert!(restarted.pause_idle(&due).await);
    let paused = restarted
        .active_leases
        .get(&lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert_eq!(paused.status, LeaseStatus::Paused);
    assert_eq!(paused.idle_pause_retry, None, "a pause clears the backoff");
    Ok(())
}

#[tokio::test]
async fn a_lease_paused_by_request_is_not_marked_as_idle_paused() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "manual-pause").await?;
    core.handle_pause_lease(PauseLeaseRequest {
        operation_id: None,
        tenant,
        lease_id: lease_id.clone(),
        max_paused_seconds: None,
    })
    .await;
    let paused = core.active_leases.get(&lease_id).await?.ok_or("lease record missing")?;
    assert_eq!(paused.status, LeaseStatus::Paused);
    assert_eq!(paused.auto_paused_at, None);
    Ok(())
}

#[tokio::test]
async fn targeted_reconciliation_pauses_then_releases_an_idle_lease() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(PauseAwareBus {
        inner: CapturingBus::new(mock_provider_response()?),
    });
    core.bus = bus.clone();
    let policy = idle_policy(&[(1, IdleAction::Pause), (3, IdleAction::Release)])?;
    let request = edit_request_spec(make_request(vec![ProviderKind::Fly])?, |parts| {
        parts.idle_policy = Some(policy);
    })?;
    let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
        return Err("fixture denied".into());
    };
    let released = || {
        locked(&bus.inner.published)
            .map(|published| {
                published
                    .iter()
                    .any(|(subject, _)| subject == &subjects::lease_released(&grant.lease_id))
            })
            .unwrap_or(false)
    };

    tokio::time::sleep(Duration::from_millis(1300)).await;
    core.reconcile_expired_leases().await;
    let paused = core
        .active_leases
        .get(&grant.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert_eq!(paused.status, LeaseStatus::Paused);
    assert!(paused.auto_paused_at.is_some());
    assert!(!released(), "pause stage must not release");

    // A second pass inside the same window does nothing more.
    core.reconcile_expired_leases().await;
    assert!(!released());

    tokio::time::sleep(Duration::from_millis(2000)).await;
    core.reconcile_expired_leases().await;
    assert!(released(), "release stage ends the paused lease");
    Ok(())
}

#[tokio::test]
async fn lease_request_validates_the_idle_policy() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    // Unsorted or conflicting policies are unrepresentable: the spec refuses to be built.
    let build = |stages: &[(u32, trogon_sandbox_contract::IdleAction)], timeout: Option<u32>| {
        let policy = idle_policy(stages)?;
        edit_request_spec(make_request(vec![ProviderKind::Mock])?, |parts| {
            parts.idle_policy = Some(policy);
            parts.idle_timeout_seconds = timeout.and_then(NonZeroU32::new);
        })
    };
    assert!(build(&[(600, Pause), (60, Release)], None).is_err());
    assert!(build(&[(60, Pause), (60, Release)], None).is_err());
    assert!(build(&[(60, Release), (600, Pause)], None).is_err());
    assert!(build(&[(60, Pause)], Some(30)).is_err());
    let ok = build(&[(60, Pause), (600, Release)], None)?;
    assert!(matches!(core.handle_lease_request(ok).await, LeaseResponse::Granted(_)));
    Ok(())
}
