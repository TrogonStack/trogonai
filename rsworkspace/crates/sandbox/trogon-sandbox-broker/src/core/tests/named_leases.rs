use super::*;

#[tokio::test]
async fn racing_named_creates_provision_one_lease() -> TestResult {
    let core = Arc::new(make_core(mock_provider_response()?)?);
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);

    let mut creators = Vec::new();
    for n in 0..6 {
        let core = core.clone();
        let mut request = request.clone();
        request.idempotency_key = LeaseRequestKey::new(format!("race-{n}"))?;
        creators.push(tokio::spawn(async move { core.handle_lease_request(request).await }));
    }
    let mut granted = Vec::new();
    for creator in creators {
        match creator.await? {
            LeaseResponse::Granted(g) => granted.push(g.lease_id),
            LeaseResponse::Denied(LeaseError::NameConflict { name }) => {
                assert_eq!(name.as_str(), "workspace");
            }
            other => return Err(format!("unexpected {other:?}").into()),
        }
    }
    let first = granted.first().ok_or("no create was granted")?;
    assert!(granted.iter().all(|id| id == first), "{granted:?}");
    assert_eq!(core.active_leases.all().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_retry_with_the_same_key_reuses_its_own_pending_name_claim() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);
    request.idempotency_key = LeaseRequestKey::new("retry-key")?;
    // A previous attempt claimed the name under the derived id, then died
    // before recording a lease.
    let derived = derived_named_lease_id(&request.tenant, "workspace", "retry-key");
    core.named_leases
        .claim(
            &request.tenant,
            "workspace",
            &derived,
            core.config.provider_dispatch_timeout,
        )
        .await?;
    match core.handle_lease_request(request.clone()).await {
        LeaseResponse::Granted(g) => assert_eq!(g.lease_id, derived),
        other => return Err(format!("unexpected {other:?}").into()),
    }
    // Confirmed after the lease was recorded, so a different key no longer
    // sees a pending claim.
    let held = core
        .named_leases
        .claim(
            &request.tenant,
            "workspace",
            &LeaseId::new("other"),
            core.config.provider_dispatch_timeout,
        )
        .await?;
    assert!(matches!(held, NameClaim::Held { pending: false, .. }), "{held:?}");
    Ok(())
}

#[tokio::test]
async fn a_retry_after_a_recorded_lease_returns_it_without_provisioning_again() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);
    request.idempotency_key = LeaseRequestKey::new("same-key")?;
    let LeaseResponse::Granted(first) = core.handle_lease_request(request.clone()).await else {
        return Err("first create must be granted".into());
    };
    // Drop the cached idempotent grant so the retry reaches the name claim.
    let derived = derived_named_lease_id(&request.tenant, "workspace", "same-key");
    assert_eq!(first.lease_id, derived);
    core.named_leases.clear(&request.tenant, "workspace").await?;
    match core.handle_lease_request(request).await {
        LeaseResponse::Granted(g) => assert_eq!(g.lease_id, first.lease_id),
        other => return Err(format!("unexpected {other:?}").into()),
    }
    assert_eq!(core.active_leases.all().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn another_keys_pending_claim_conflicts_until_it_expires() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);
    let ghost = LeaseId::new("ghost");
    core.named_leases
        .claim(
            &request.tenant,
            "workspace",
            &ghost,
            core.config.provider_dispatch_timeout,
        )
        .await?;
    let response = core.handle_lease_request(request.clone()).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::NameConflict { .. })),
        "{response:?}"
    );
    // An expired pending marker no longer blocks the name.
    core.named_leases.clear(&request.tenant, "workspace").await?;
    core.named_leases
        .claim(&request.tenant, "workspace", &ghost, Duration::from_millis(1))
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    request.idempotency_key = LeaseRequestKey::new("another-key")?;
    let response = core.handle_lease_request(request).await;
    assert!(matches!(response, LeaseResponse::Granted(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn a_failed_grant_releases_the_claimed_name() -> TestResult {
    let core = make_core(Bytes::from_static(b"not a provider response"))?;
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);
    let tenant = request.tenant.clone();
    let response = core.handle_lease_request(request).await;
    assert!(matches!(response, LeaseResponse::Denied(_)), "{response:?}");
    assert!(core.named_leases.get(&tenant, "workspace").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn delayed_named_delete_cannot_remove_a_newer_lease_mapping() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let store = Arc::new(GatedNamedClear {
        inner: Default::default(),
        gate_once: true.into(),
        entered: Default::default(),
        resume: Default::default(),
    });
    core.named_leases = store.clone();
    let core = Arc::new(core);
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = LeasePersistence::from(false);
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(old) = core.handle_lease_request(request).await else {
        return Err("fixture denied".into());
    };
    let deleting = tokio::spawn({
        let core = core.clone();
        let tenant = tenant.clone();
        async move {
            core.handle_delete_lease(DeleteLeaseRequest {
                tenant,
                lease_id: old.lease_id,
                operation_id: None,
            })
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), store.entered.notified()).await?;
    let newer = LeaseId::new("newer-named-lease");
    store.inner.set(&tenant, "workspace", &newer).await?;
    store.resume.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(2), deleting).await??;
    assert!(matches!(response, DeleteLeaseResponse::Deleted { .. }), "{response:?}");
    assert_eq!(store.inner.get(&tenant, "workspace").await?, Some(newer));
    Ok(())
}

#[tokio::test]
async fn reconciliation_uses_the_journal_deadline_when_a_replica_writes_a_stale_index() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    core.bus = bus.clone();
    let LeaseResponse::Granted(granted) = core.handle_lease_request(make_request(vec![ProviderKind::Fly])?).await
    else {
        return Err("fixture denied".into());
    };
    let mut stale = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("the granted lease is missing")?;
    stale.deadline = SystemTime::UNIX_EPOCH;
    core.active_leases.put(&stale).await?;
    core.reconcile_expired_leases().await;
    let current = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("the granted lease is missing")?;
    assert_eq!(
        current.status,
        LeaseStatus::Running,
        "a stale index cannot expire an unexpired durable lease"
    );
    assert_eq!(current.deadline, granted.deadline);
    Ok(())
}
