use super::*;

use trogon_sandbox_contract::{
    IdleAction, LeaseTtl, ReapLeasesRequest, ReapLeasesRequestError, ReapMode, SandboxSpecError,
};

/// A reap request for `tenant` with the given tag filter, age and idleness bounds.
fn reap(
    tenant: &TenantId,
    tags: &[(&str, &str)],
    older_than_seconds: Option<u32>,
    idle_for_seconds: Option<u32>,
    mode: ReapMode,
) -> anyhow::Result<ReapLeasesRequest> {
    Ok(ReapLeasesRequest::new(
        tenant.clone(),
        SandboxTags::new(tag_filter(tags))?,
        older_than_seconds,
        idle_for_seconds,
        mode,
    )?)
}

/// `request` with its idle timeout set to `seconds`.
fn with_idle_timeout(request: LeaseRequest, seconds: u32) -> anyhow::Result<LeaseRequest> {
    edit_request_spec(request, |parts| {
        parts.idle_timeout_seconds = NonZeroU32::new(seconds);
    })
}

/// `request` asking for an unbounded lease that idles out after `seconds`.
fn unbounded_with_idle_timeout(request: LeaseRequest, seconds: u32) -> anyhow::Result<LeaseRequest> {
    edit_request_spec(request, |parts| {
        parts.no_ttl = LeaseTtl::Unbounded;
        parts.idle_timeout_seconds = NonZeroU32::new(seconds);
    })
}

#[tokio::test]
async fn granted_lease_record_keeps_the_requested_tags() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tags = SandboxTags::new(tag_filter(&[("app", "web")]))?;

    let LeaseResponse::Granted(grant) = core.handle_lease_request(req).await else {
        return Err("expected a grant".into());
    };

    let record = core
        .active_leases
        .get(&grant.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert_eq!(record.tags.get("app"), Some("web"));
    Ok(())
}

#[test]
fn a_zero_idle_timeout_cannot_be_constructed() {
    assert!(NonZeroU32::new(0).is_none());
}

#[tokio::test]
async fn a_ttl_over_the_default_lifetime_cap_is_denied() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.ttl_seconds = MAX_LEASE_TTL_SECONDS + 1;

    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::WouldExceedMaxLifetime { max })
            if max == MAX_LEASE_TTL_SECONDS
    ));
    Ok(())
}

#[test]
fn a_raised_lifetime_cap_admits_a_two_day_ttl() -> TestResult {
    let policy = LifetimePolicy {
        max_lease_lifetime_seconds: 172_800,
        allow_no_ttl: false,
    };
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.ttl_seconds = 172_800;
    assert_eq!(policy.check(&req), Ok(()));
    req.ttl_seconds = 172_801;
    assert_eq!(
        policy.check(&req),
        Err(LeaseError::WouldExceedMaxLifetime { max: 172_800 })
    );
    Ok(())
}

#[tokio::test]
async fn a_no_ttl_lease_is_denied_unless_the_operator_allows_it() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = unbounded_with_idle_timeout(make_request(vec![ProviderKind::Mock])?, 60)?;

    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::InvalidSpec { reason }) if reason.contains("no_ttl")
    ));
    Ok(())
}

#[test]
fn a_no_ttl_lease_needs_an_idle_timeout() -> TestResult {
    let error = edit_request_spec(make_request(vec![ProviderKind::Mock])?, |parts| {
        parts.no_ttl = LeaseTtl::Unbounded;
    })
    .err()
    .ok_or("a no_ttl spec without an idle limit must not build")?;
    assert!(
        matches!(
            error.downcast_ref::<SandboxSpecError>(),
            Some(SandboxSpecError::NoTtlWithoutIdleRelease)
        ),
        "expected NoTtlWithoutIdleRelease, got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_idle_policy_release_stage_satisfies_a_no_ttl_lease() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_lifetime_policy(LifetimePolicy {
        allow_no_ttl: true,
        max_lease_lifetime_seconds: 3_600,
    });
    let request = |stages: &[(u32, IdleAction)]| -> anyhow::Result<LeaseRequest> {
        let policy = idle_policy(stages)?;
        edit_request_spec(make_request(vec![ProviderKind::Mock])?, |parts| {
            parts.no_ttl = LeaseTtl::Unbounded;
            parts.idle_policy = Some(policy);
        })
    };

    let error = request(&[(60, IdleAction::Pause)])
        .err()
        .ok_or("a pause-only idle policy must not satisfy a no_ttl spec")?;
    assert!(
        matches!(
            error.downcast_ref::<SandboxSpecError>(),
            Some(SandboxSpecError::NoTtlWithoutIdleRelease)
        ),
        "expected NoTtlWithoutIdleRelease, got {error:?}"
    );
    let granted = core
        .handle_lease_request(request(&[(60, IdleAction::Pause), (600, IdleAction::Release)])?)
        .await;
    assert!(matches!(granted, LeaseResponse::Granted(_)), "{granted:?}");
    Ok(())
}

#[tokio::test]
async fn an_allowed_no_ttl_lease_gets_a_far_deadline_and_keeps_its_idle_limit() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_lifetime_policy(LifetimePolicy {
        allow_no_ttl: true,
        max_lease_lifetime_seconds: 3_600,
    });
    let req = unbounded_with_idle_timeout(make_request(vec![ProviderKind::Mock])?, 60)?;

    let LeaseResponse::Granted(granted) = core.handle_lease_request(req).await else {
        return Err("an allowed no_ttl lease should be granted".into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("granted lease should be recorded")?;
    let ahead = record.deadline.duration_since(SystemTime::now()).unwrap_or_default();
    assert!(ahead > Duration::from_secs(u64::from(MAX_LEASE_TTL_SECONDS) * 365));
    assert_eq!(record.spec.idle_timeout_seconds(), NonZeroU32::new(60));
    Ok(())
}

#[tokio::test]
async fn a_no_ttl_lease_is_charged_the_lifetime_cap_and_refunds_it() -> TestResult {
    let cap = 7_200;
    let core = make_core(mock_provider_response()?)?.with_lifetime_policy(LifetimePolicy {
        allow_no_ttl: true,
        max_lease_lifetime_seconds: cap,
    });
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.ttl_seconds = 60;
    let req = unbounded_with_idle_timeout(req, 60)?;
    let tenant = req.tenant.clone();

    let LeaseResponse::Granted(granted) = core.handle_lease_request(req).await else {
        return Err("an allowed no_ttl lease should be granted".into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("granted lease should be recorded")?;
    assert_eq!(record.wall_seconds_consumed, cap);
    let quota = core.quota.quota_record(&tenant).await?.ok_or("quota record missing")?;
    assert_eq!(quota.wall_seconds_used, cap);
    Ok(())
}

#[tokio::test]
async fn a_no_ttl_lease_is_refused_when_the_lifetime_cap_exceeds_the_wall_budget() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_lifetime_policy(LifetimePolicy {
        allow_no_ttl: true,
        ..LifetimePolicy::default()
    });
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.ttl_seconds = 60;
    let req = unbounded_with_idle_timeout(req, 60)?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::QuotaExceeded { .. })),
        "got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn extend_respects_a_raised_lifetime_cap() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?.with_lifetime_policy(LifetimePolicy {
        max_lease_lifetime_seconds: 172_800,
        allow_no_ttl: false,
    });
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, MAX_LEASE_TTL_SECONDS + 1))
        .await;
    assert!(
        matches!(response, ExtendLeaseTimeoutResponse::Extended { .. }),
        "a raised cap should admit the extension, got {response:?}"
    );

    let response = core
        .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, 172_800))
        .await;
    assert!(matches!(
        response,
        ExtendLeaseTimeoutResponse::Denied(LeaseError::WouldExceedMaxLifetime { max: 172_800 })
    ));
    Ok(())
}

#[tokio::test]
async fn extending_a_no_ttl_lease_leaves_its_deadline_alone() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.spec = with_spec_parts(record.spec, |parts| {
        parts.no_ttl = LeaseTtl::Unbounded;
        parts.idle_timeout_seconds = NonZeroU32::new(60);
    })?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, MAX_LEASE_TTL_SECONDS * 4))
        .await;
    assert!(matches!(
        response,
        ExtendLeaseTimeoutResponse::Extended { new_deadline } if new_deadline == record.deadline
    ));
    Ok(())
}

#[tokio::test]
async fn list_leases_filters_by_every_tag_and_reports_tags() -> TestResult {
    let tenant = TenantId::new("tenant-a")?;
    let core = make_core(mock_provider_response()?)?;
    let now = SystemTime::now();
    let both = tagged(
        lease_record_at(&tenant, &LeaseId::new("l-both"), LeaseStatus::Running, now)?,
        &[("app", "web"), ("env", "dev")],
    )?;
    let one = tagged(
        lease_record_at(&tenant, &LeaseId::new("l-one"), LeaseStatus::Running, now)?,
        &[("app", "web")],
    )?;
    let none = lease_record_at(&tenant, &LeaseId::new("l-none"), LeaseStatus::Running, now)?;
    for record in [&both, &one, &none] {
        core.active_leases.put(record).await?;
    }
    let list = |tags: &[(&str, &str)]| -> anyhow::Result<ListLeasesRequest> {
        Ok(ListLeasesRequest {
            tenant: tenant.clone(),
            status_filter: None,
            cursor: None,
            limit: None,
            tags: SandboxTags::new(tag_filter(tags))?,
        })
    };

    let web = core.handle_list_leases(list(&[("app", "web")])?).await;
    assert_eq!(web.leases.len(), 2);
    let both_only = core.handle_list_leases(list(&[("app", "web"), ("env", "dev")])?).await;
    assert_eq!(both_only.leases.len(), 1);
    assert_eq!(both_only.leases[0].lease_id.as_str(), "l-both");
    assert_eq!(both_only.leases[0].tags.get("env"), Some("dev"));
    let wrong_value = core.handle_list_leases(list(&[("app", "api")])?).await;
    assert!(wrong_value.leases.is_empty());
    let unfiltered = core.handle_list_leases(list(&[])?).await;
    assert_eq!(unfiltered.leases.len(), 3);
    Ok(())
}

#[test]
fn reap_without_any_filter_cannot_be_constructed() -> TestResult {
    let tenant = TenantId::new("tenant-a")?;
    assert!(matches!(
        reap(&tenant, &[], None, None, ReapMode::Release)
            .err()
            .and_then(|error| error.downcast::<ReapLeasesRequestError>().ok()),
        Some(ReapLeasesRequestError::NoFilter)
    ));
    Ok(())
}

#[tokio::test]
async fn reap_dry_run_reports_matches_and_releases_nothing() -> TestResult {
    let tenant = TenantId::new("tenant-a")?;
    let core = make_core(mock_provider_response()?)?;
    let record = tagged(
        lease_record_at(&tenant, &LeaseId::new("l-1"), LeaseStatus::Running, SystemTime::now())?,
        &[("app", "web")],
    )?;
    core.active_leases.put(&record).await?;
    let req = reap(&tenant, &[("app", "web")], None, None, ReapMode::DryRun)?;

    let ReapLeasesResponse::Reaped {
        dry_run,
        matched,
        released,
        failed,
    } = core.handle_reap_leases(req).await
    else {
        return Err("expected Reaped".into());
    };

    assert!(dry_run.is_dry_run());
    assert_eq!(matched.len(), 1);
    assert!(released.is_empty() && failed.is_empty());
    assert!(core.active_leases.get(&record.lease_id).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn reap_releases_only_matching_leases_of_the_tenant_and_is_idempotent() -> TestResult {
    let tenant = TenantId::new("tenant-a")?;
    let other = TenantId::new("tenant-b")?;
    let core = make_core(mock_provider_response()?)?;
    let now = SystemTime::now();
    let target = tagged(
        lease_record_at(&tenant, &LeaseId::new("target"), LeaseStatus::Running, now)?,
        &[("app", "web")],
    )?;
    let other_app = tagged(
        lease_record_at(&tenant, &LeaseId::new("other-app"), LeaseStatus::Running, now)?,
        &[("app", "api")],
    )?;
    let other_tenant = tagged(
        lease_record_at(&other, &LeaseId::new("other-tenant"), LeaseStatus::Running, now)?,
        &[("app", "web")],
    )?;
    for record in [&target, &other_app, &other_tenant] {
        core.active_leases.put(record).await?;
    }
    let req = reap(&tenant, &[("app", "web")], None, None, ReapMode::Release)?;

    let ReapLeasesResponse::Reaped { released, .. } = core.handle_reap_leases(req.clone()).await else {
        return Err("expected Reaped".into());
    };
    assert_eq!(released, vec![target.lease_id.clone()]);
    assert!(core.active_leases.get(&target.lease_id).await?.is_none());
    assert!(core.active_leases.get(&other_app.lease_id).await?.is_some());
    assert!(core.active_leases.get(&other_tenant.lease_id).await?.is_some());

    let ReapLeasesResponse::Reaped {
        matched,
        released,
        failed,
        ..
    } = core.handle_reap_leases(req).await
    else {
        return Err("expected Reaped".into());
    };
    assert!(matched.is_empty() && released.is_empty() && failed.is_empty());
    Ok(())
}

#[tokio::test]
async fn reap_by_age_and_idleness_uses_creation_and_last_activity() -> TestResult {
    let tenant = TenantId::new("tenant-a")?;
    let core = make_core(mock_provider_response()?)?;
    let now = SystemTime::now();
    let hour = Duration::from_secs(3600);
    let old_idle = lease_record_at(&tenant, &LeaseId::new("old-idle"), LeaseStatus::Running, now - hour)?;
    let mut old_active = lease_record_at(&tenant, &LeaseId::new("old-active"), LeaseStatus::Running, now - hour)?;
    old_active.last_activity_at = Some(now);
    let young = lease_record_at(&tenant, &LeaseId::new("young"), LeaseStatus::Running, now)?;
    for record in [&old_idle, &old_active, &young] {
        core.active_leases.put(record).await?;
    }

    let by_age = reap(&tenant, &[], Some(1800), None, ReapMode::DryRun)?;
    let ReapLeasesResponse::Reaped { matched, .. } = core.handle_reap_leases(by_age).await else {
        return Err("expected Reaped".into());
    };
    assert_eq!(matched.len(), 2);

    let by_idle = reap(&tenant, &[], None, Some(1800), ReapMode::DryRun)?;
    let ReapLeasesResponse::Reaped { matched, .. } = core.handle_reap_leases(by_idle).await else {
        return Err("expected Reaped".into());
    };
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].lease_id.as_str(), "old-idle");
    Ok(())
}

#[tokio::test]
async fn routed_verbs_record_activity_only_for_leases_with_an_idle_timeout() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let core = make_core(mock_provider_response()?)?;
    let long_ago = SystemTime::now() - Duration::from_secs(600);
    let mut tracked = running_source_record(&tenant, &LeaseId::new("tracked"))?;
    tracked.spec = with_spec_parts(tracked.spec, |parts| {
        parts.idle_timeout_seconds = NonZeroU32::new(300);
    })?;
    tracked.created_at = long_ago;
    let mut untracked = running_source_record(&tenant, &LeaseId::new("untracked"))?;
    untracked.created_at = long_ago;
    core.active_leases.put(&tracked).await?;
    core.active_leases.put(&untracked).await?;

    for lease_id in [&tracked.lease_id, &untracked.lease_id] {
        let _ = core
            .handle_poll_command(PollCommandRequest {
                lease_id: lease_id.clone(),
                execution_id: ExecutionId::new("exec-1"),
            })
            .await;
    }

    let tracked = core
        .active_leases
        .get(&tracked.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert!(tracked.last_activity_at.is_some_and(|seen| seen > long_ago));
    let untracked = core
        .active_leases
        .get(&untracked.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert_eq!(untracked.last_activity_at, None);
    Ok(())
}

#[tokio::test]
async fn a_worker_activity_signal_counts_toward_the_idle_timeout() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let core = make_core(mock_provider_response()?)?;
    let long_ago = SystemTime::now() - Duration::from_secs(600);
    let mut record = running_source_record(&tenant, &LeaseId::new("terminal-only"))?;
    record.spec = with_spec_parts(record.spec, |parts| {
        parts.idle_timeout_seconds = NonZeroU32::new(300);
    })?;
    record.created_at = long_ago;
    core.active_leases.put(&record).await?;

    core.handle_worker_activity(&record.lease_id).await;
    // An unknown lease is ignored.
    core.handle_worker_activity(&LeaseId::new("gone")).await;

    let record = core
        .active_leases
        .get(&record.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert!(record.last_activity_at.is_some_and(|seen| seen > long_ago));
    Ok(())
}

#[tokio::test]
async fn targeted_leases_stamp_activity_into_the_attempt_journal() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    core.bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let request = with_idle_timeout(make_request(vec![ProviderKind::Fly])?, 300)?;
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request).await else {
        return Err("fixture denied".into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert!(record.dispatch_attempt.is_some());
    let key = record
        .dispatch_attempt
        .as_ref()
        .ok_or("dispatch attempt missing")?
        .idempotency_key
        .clone();
    core.touch_activity(&record, true).await;
    let journal = core
        .idempotency
        .get_attempt(&record.tenant, &key)
        .await?
        .ok_or("lease record missing")?;
    assert!(journal.last_activity_at.is_some());
    let indexed = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert!(indexed.last_activity_at.is_some());
    // Reconciliation copies the journal over the index, so the stamp survives.
    core.reconcile_expired_leases().await;
    let indexed = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("lease record missing")?;
    assert_eq!(indexed.last_activity_at, journal.last_activity_at);
    assert_eq!(indexed.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn targeted_reconciliation_releases_idle_leases_and_spares_active_ones() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    core.bus = bus.clone();
    let mut idle_request = with_idle_timeout(make_request(vec![ProviderKind::Fly])?, 1)?;
    idle_request.idempotency_key = LeaseRequestKey::new("idle")?;
    let LeaseResponse::Granted(idle) = core.handle_lease_request(idle_request).await else {
        return Err("fixture denied".into());
    };
    let mut busy_request = with_idle_timeout(make_request(vec![ProviderKind::Fly])?, 1)?;
    busy_request.idempotency_key = LeaseRequestKey::new("busy")?;
    let LeaseResponse::Granted(busy) = core.handle_lease_request(busy_request).await else {
        return Err("fixture denied".into());
    };
    // Both sit quiet past the idle limit; only the busy one then shows activity.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let busy_record = core
        .active_leases
        .get(&busy.lease_id)
        .await?
        .ok_or("lease record missing")?;
    core.touch_activity(&busy_record, true).await;

    core.reconcile_expired_leases().await;

    let published = locked(&bus.published)?;
    let released = |lease_id: &LeaseId| {
        published
            .iter()
            .find(|(subject, _)| subject == &subjects::lease_released(lease_id))
            .and_then(|(_, payload)| serde_json::from_slice::<LeaseReleased>(payload).ok())
    };
    assert_eq!(
        released(&idle.lease_id).map(|event| event.reason),
        Some(LeaseReleasedReason::IdleTimeout)
    );
    assert!(released(&busy.lease_id).is_none());
    Ok(())
}

#[tokio::test]
async fn list_leases_returns_only_tenant_leases() -> TestResult {
    let tenant_a = TenantId::new("tenant-a")?;
    let tenant_b = TenantId::new("tenant-b")?;
    let core = make_core(mock_provider_response()?)?;
    let now = SystemTime::now();

    for i in 0..3 {
        let record = lease_record_at(
            &tenant_a,
            &LeaseId::new(format!("lease-a-{i}")),
            LeaseStatus::Running,
            now + Duration::from_secs(i),
        )?;
        core.active_leases.put(&record).await?;
    }
    for i in 0..2 {
        let record = lease_record_at(
            &tenant_b,
            &LeaseId::new(format!("lease-b-{i}")),
            LeaseStatus::Running,
            now + Duration::from_secs(i),
        )?;
        core.active_leases.put(&record).await?;
    }

    let response = core
        .handle_list_leases(ListLeasesRequest {
            tenant: tenant_a.clone(),
            status_filter: None,
            cursor: None,
            limit: None,
            tags: Default::default(),
        })
        .await;

    assert_eq!(response.leases.len(), 3);
    assert!(response.leases.iter().all(|summary| summary.tenant == tenant_a));
    assert!(response.leases.iter().all(|summary| summary.tenant != tenant_b));
    assert_eq!(response.next_cursor, None);
    Ok(())
}

#[tokio::test]
async fn list_leases_paginates_stably_across_calls() -> TestResult {
    let tenant = TenantId::new("tenant-a")?;
    let core = make_core(mock_provider_response()?)?;
    let now = SystemTime::now();

    for i in 0..5 {
        let record = lease_record_at(
            &tenant,
            &LeaseId::new(format!("lease-{i}")),
            LeaseStatus::Running,
            now + Duration::from_secs(i),
        )?;
        core.active_leases.put(&record).await?;
    }

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;

    let page1 = core
        .handle_list_leases(ListLeasesRequest {
            tenant: tenant.clone(),
            status_filter: None,
            cursor: cursor.clone(),
            limit: Some(2),
            tags: Default::default(),
        })
        .await;
    assert_eq!(page1.leases.len(), 2);
    assert!(page1.next_cursor.is_some());
    seen.extend(page1.leases.iter().map(|s| s.lease_id.as_str().to_string()));
    cursor = page1.next_cursor;

    let page2 = core
        .handle_list_leases(ListLeasesRequest {
            tenant: tenant.clone(),
            status_filter: None,
            cursor: cursor.clone(),
            limit: Some(2),
            tags: Default::default(),
        })
        .await;
    assert_eq!(page2.leases.len(), 2);
    assert!(page2.next_cursor.is_some());
    seen.extend(page2.leases.iter().map(|s| s.lease_id.as_str().to_string()));
    cursor = page2.next_cursor;

    let page3 = core
        .handle_list_leases(ListLeasesRequest {
            tenant: tenant.clone(),
            status_filter: None,
            cursor: cursor.clone(),
            limit: Some(2),
            tags: Default::default(),
        })
        .await;
    assert_eq!(page3.leases.len(), 1);
    assert_eq!(page3.next_cursor, None);
    seen.extend(page3.leases.iter().map(|s| s.lease_id.as_str().to_string()));

    let unique: std::collections::HashSet<_> = seen.iter().cloned().collect();
    assert_eq!(unique.len(), 5, "no duplicate or skipped lease_id across pages");
    assert_eq!(seen.len(), 5);
    Ok(())
}

#[tokio::test]
async fn delete_drive_rejects_another_tenants_id_before_storage_purge() -> TestResult {
    let owner = TenantId::new("drive-owner")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&owner, "private-data")?;
    drive_store
        .create(&owner, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let purger = Arc::new(RecordingDriveDataPurger::new());
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?
        .with_drive_data_purger(purger.clone());

    let response = core
        .handle_delete_drive(DeleteDriveRequest {
            tenant: TenantId::new("other-tenant")?,
            drive_id: drive_id.clone(),
        })
        .await;

    assert!(matches!(response, DeleteDriveResponse::Denied(_)), "{response:?}");
    assert!(purger.purged_records()?.is_empty());
    assert!(drive_store.get(&drive_id).await?.is_some());
    Ok(())
}
