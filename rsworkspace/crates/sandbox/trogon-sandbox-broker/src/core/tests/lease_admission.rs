use super::*;

fn registry_image(reference: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(reference),
        pull_secret: None,
    }
}

#[tokio::test]
async fn grant_emits_broker_metric_samples() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));

    let text = core.metrics().gather_text()?;
    assert!(text.contains("sandbox_broker_lease_dispatch_total"));
    assert!(text.contains("sandbox_broker_active_leases"));
    Ok(())
}

#[tokio::test]
async fn empty_image_returns_invalid_spec() -> TestResult {
    // An empty image is unrepresentable: the spec refuses to be built.
    let req = make_request(vec![ProviderKind::Mock])?;
    assert!(with_spec_parts(req.spec, |parts| parts.image = registry_image("")).is_err());
    Ok(())
}

#[tokio::test]
async fn no_provider_pref_returns_no_provider_available() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_request(vec![])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::NoProviderAvailable)
    ));
    Ok(())
}

#[tokio::test]
async fn quota_exhaustion_returns_quota_exceeded() -> TestResult {
    let bus = Arc::new(MockBus {
        provider_response: mock_provider_response()?,
    });
    let quota = Arc::new(InMemoryQuotaStore::new());
    let core = BrokerCore::new(
        bus,
        quota,
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
        default_pins(),
        default_tag_store(),
        default_resolved_digests(),
        default_drive_store(),
        default_named_leases(),
        BrokerConfig {
            default_cpu_cap: 50,
            default_wall_cap: 7200,
            enforce_image_allowlist: false,
            enforce_worker_registry: false,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 50,
            snapshot_registry: "snapshots.trogon.internal".to_string(),
            provider_dispatch_timeout: Duration::from_secs(600),
        },
        BrokerMetrics::new()?,
    );
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::QuotaExceeded { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn same_idempotency_key_returns_cached_without_double_charge() -> TestResult {
    let (core, count) = make_core_with_counting_quota(mock_provider_response()?)?;
    let req = make_request(vec![ProviderKind::Mock])?;

    let first = core.handle_lease_request(req.clone()).await;
    assert!(matches!(first, LeaseResponse::Granted(_)));
    assert_eq!(count.load(Ordering::SeqCst), 1, "quota charged once on first");

    let second = core.handle_lease_request(req).await;
    assert!(matches!(second, LeaseResponse::Granted(_)));
    // Quota must NOT be called a second time
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "quota NOT charged on idempotent replay"
    );

    // Both responses share the same lease_id
    let (LeaseResponse::Granted(g1), LeaseResponse::Granted(g2)) = (first, second) else {
        return Err("both responses must be grants".into());
    };
    assert_eq!(g1.lease_id, g2.lease_id);
    assert!(g1.created.is_created(), "first grant creates the lease");
    assert!(!g2.created.is_created(), "idempotent replay does not create");
    Ok(())
}

#[tokio::test]
async fn a_racing_duplicate_with_one_key_returns_the_first_lease_and_releases_its_own() -> TestResult {
    let core = BrokerCore::new(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(RacingIdempotencyStore(InMemoryIdempotencyStore::new())),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
        default_pins(),
        default_tag_store(),
        default_resolved_digests(),
        default_drive_store(),
        default_named_leases(),
        default_config(),
        BrokerMetrics::new()?,
    );
    let req = make_request(vec![ProviderKind::Mock])?;

    let LeaseResponse::Granted(first) = core.handle_lease_request(req.clone()).await else {
        return Err("first request must be granted".into());
    };
    let LeaseResponse::Granted(second) = core.handle_lease_request(req).await else {
        return Err("racing duplicate must be granted".into());
    };

    assert_eq!(second.lease_id, first.lease_id, "both callers share one lease");
    assert!(
        !second.created.is_created(),
        "the duplicate did not create the lease it returns"
    );
    let live: Vec<LeaseId> = core
        .active_leases
        .all()
        .await?
        .into_iter()
        .map(|record| record.lease_id)
        .collect();
    assert_eq!(live, vec![first.lease_id], "the duplicate lease was released");
    Ok(())
}

#[tokio::test]
async fn named_reconnect_reports_created_false() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);

    let LeaseResponse::Granted(first) = core.handle_lease_request(request.clone()).await else {
        return Err("first named lease must be granted".into());
    };
    assert!(first.created.is_created());

    request.idempotency_key = LeaseRequestKey::new("second-key")?;
    let LeaseResponse::Granted(second) = core.handle_lease_request(request).await else {
        return Err("named reconnect must be granted".into());
    };
    assert_eq!(second.lease_id, first.lease_id);
    assert!(!second.created.is_created(), "reconnect must not report created");
    Ok(())
}

#[tokio::test]
async fn different_idempotency_keys_charge_quota_twice() -> TestResult {
    let (core, count) = make_core_with_counting_quota(mock_provider_response()?)?;

    let mut req1 = make_request(vec![ProviderKind::Mock])?;
    req1.idempotency_key = LeaseRequestKey::new("key-a")?;

    let mut req2 = make_request(vec![ProviderKind::Mock])?;
    req2.idempotency_key = LeaseRequestKey::new("key-b")?;

    let r1 = core.handle_lease_request(req1).await;
    let r2 = core.handle_lease_request(req2).await;

    assert!(matches!(r1, LeaseResponse::Granted(_)));
    assert!(matches!(r2, LeaseResponse::Granted(_)));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn malformed_idempotency_key_is_unrepresentable() {
    assert!(LeaseRequestKey::new("bad/key").is_err());
}

#[tokio::test]
async fn tenant_with_no_record_and_enforce_true_is_denied() -> TestResult {
    let allowlist = Arc::new(InMemoryImageAllowList::new());
    let core = make_core_with_allowlist(mock_provider_response()?, allowlist, true).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn tenant_with_matching_pattern_is_granted() -> TestResult {
    let allowlist = Arc::new(InMemoryImageAllowList::new());
    let tenant = TenantId::new("test-tenant")?;
    allowlist
        .insert(
            &tenant,
            TenantImagePolicy::from_patterns(vec!["ubuntu:24.04".to_string()]),
        )
        .await;
    let core = make_core_with_allowlist(mock_provider_response()?, allowlist, true).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn tenant_with_non_matching_pattern_is_denied() -> TestResult {
    let allowlist = Arc::new(InMemoryImageAllowList::new());
    let tenant = TenantId::new("test-tenant")?;
    allowlist
        .insert(&tenant, TenantImagePolicy::from_patterns(vec!["debian:12".to_string()]))
        .await;
    let core = make_core_with_allowlist(mock_provider_response()?, allowlist, true).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn enforce_false_bypasses_allowlist() -> TestResult {
    let allowlist = Arc::new(InMemoryImageAllowList::new());
    let core = make_core_with_allowlist(mock_provider_response()?, allowlist, false).await?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.spec = with_spec_parts(req.spec, |parts| parts.image = registry_image("any/thing:whatever"))?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn incompatible_platform_image_is_denied() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_image_readiness(Arc::new(IncompatiblePlatformChecker));
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::ImageNotReady { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn preparing_image_is_denied() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_image_readiness(Arc::new(PreparingChecker));
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::ImageNotReady { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn readiness_checker_error_fails_open() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_image_readiness(Arc::new(FailingChecker));
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn noop_readiness_checker_grants_by_default() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}
