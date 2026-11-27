use super::*;
use trogon_sandbox_contract::LaunchKey;

struct E2bAssignmentBus;

#[async_trait::async_trait]
impl crate::bus::BrokerBus for E2bAssignmentBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
        let target = dispatch.target;
        Ok(serde_json::to_vec(&CoreProviderLease {
            sandbox_id: SandboxId::new("owned-e2b-assignment")?,
            evidence_subject: "owned-e2b-evidence".into(),
            epoch: target.as_ref().and_then(|target| target.epoch),
            worker_id: target.as_ref().map(|target| target.worker_id.clone()),
            attempt_id: target.map(|target| target.attempt_id),
        })?
        .into())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn ordinary_e2b_create_retains_targeted_assignment_with_registry_unenforced() -> TestResult {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::E2b;
    let mut heartbeat = worker_heartbeat("assigned-e2b-worker", ProviderKind::E2b, profile)?;
    heartbeat.supports_lease_epoch = CapabilitySupport::Supported;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    let mut core = make_core_with_registry(mock_provider_response()?, registry, false)?;
    core.bus = Arc::new(E2bAssignmentBus);
    let request = make_request(vec![ProviderKind::E2b])?;
    let response = core.handle_lease_request(request).await;
    let LeaseResponse::Granted(grant) = response else {
        return Err(format!("the ordinary E2B create must reach its configured worker: {response:?}").into());
    };
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("record missing")?;
    assert_eq!(
        record.worker_id.as_ref().map(|worker| worker.as_str()),
        Some("assigned-e2b-worker"),
        "registry enforcement must not erase an available targeted assignment"
    );
    assert!(record.dispatch_attempt.is_some() && record.epoch.is_some());
    Ok(())
}

#[tokio::test]
async fn reconciler_redrives_a_pending_targeted_create_whose_reply_was_lost() -> TestResult {
    let (mut core, _bus, _drive, request) = native_failure_fixture(None).await?;
    let bus = Arc::new(LostReplyWorkerBus {
        launched: Default::default(),
        sends: AtomicU32::new(0),
        lose_first_reply: true.into(),
    });
    core.bus = bus.clone();
    core.config.provider_dispatch_timeout = Duration::ZERO;

    let first = core.handle_lease_request(request.clone()).await;
    assert!(matches!(first, LeaseResponse::Denied(_)), "{first:?}");
    let records = core.active_leases.all().await?;
    assert_eq!(records[0].status, LeaseStatus::Pending);

    core.reconcile_expired_leases().await;

    let record = core
        .active_leases
        .get(&records[0].lease_id)
        .await?
        .ok_or("record missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(record.sandbox_id, "machine-1");
    assert_eq!(bus.sends.load(Ordering::SeqCst), 2);
    assert_eq!(
        bus.launched.lock().map_err(|_| "poisoned lock")?.len(),
        1,
        "one launch only"
    );
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request).await else {
        return Err("the retried create must now replay the grant".into());
    };
    assert_eq!(granted.lease_id, record.lease_id);
    assert_eq!(bus.sends.load(Ordering::SeqCst), 2);

    core.reconcile_expired_leases().await;
    assert_eq!(bus.sends.load(Ordering::SeqCst), 2, "Running is not re-sent");
    Ok(())
}

#[tokio::test]
async fn a_restarted_broker_redrives_the_journaled_dispatch() -> TestResult {
    let (mut core, _bus, _drive, request) = native_failure_fixture(None).await?;
    let bus = Arc::new(LostReplyWorkerBus {
        launched: Default::default(),
        sends: AtomicU32::new(0),
        lose_first_reply: true.into(),
    });
    core.bus = bus.clone();
    core.config.provider_dispatch_timeout = Duration::ZERO;

    let first = core.handle_lease_request(request.clone()).await;
    assert!(matches!(first, LeaseResponse::Denied(_)), "{first:?}");
    let pending = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    assert!(
        pending
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.pending_dispatch.is_some()),
        "the sent dispatch must be journaled while Pending"
    );
    // A restart loses everything the process held in memory.
    core.pending_redrives.lock().map_err(|_| "poisoned lock")?.clear();

    core.reconcile_expired_leases().await;

    let record = core
        .active_leases
        .get(&pending.lease_id)
        .await?
        .ok_or("record missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(bus.sends.load(Ordering::SeqCst), 2);
    assert_eq!(
        bus.launched.lock().map_err(|_| "poisoned lock")?.len(),
        1,
        "one launch only"
    );
    assert!(
        record
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.pending_dispatch.is_none()),
        "a committed attempt drops its journaled dispatch"
    );
    Ok(())
}

#[tokio::test]
async fn only_one_replica_redrives_a_journaled_dispatch_per_timeout() -> TestResult {
    let (mut core, _bus, _drive, request) = native_failure_fixture(None).await?;
    let bus = Arc::new(LostReplyWorkerBus {
        launched: Default::default(),
        sends: AtomicU32::new(0),
        lose_first_reply: true.into(),
    });
    core.bus = bus.clone();
    core.config.provider_dispatch_timeout = Duration::ZERO;
    let first = core.handle_lease_request(request).await;
    assert!(matches!(first, LeaseResponse::Denied(_)), "{first:?}");
    let pending = core.active_leases.all().await?.pop().ok_or("no lease record")?;
    assert_eq!(bus.sends.load(Ordering::SeqCst), 1);

    // Age the journaled send so the dispatch timeout has certainly passed.
    let attempt = pending.dispatch_attempt.as_ref().ok_or("missing value")?;
    let mut aged = pending.clone();
    aged.dispatch_attempt
        .as_mut()
        .ok_or("missing value")?
        .pending_dispatch
        .as_mut()
        .ok_or("missing value")?
        .sent_at = SystemTime::now() - Duration::from_secs(7200);
    assert!(
        core.idempotency
            .replace_attempt(&pending.tenant, &attempt.idempotency_key, &pending, &aged)
            .await?
    );
    core.config.provider_dispatch_timeout = Duration::from_secs(3600);
    bus.lose_first_reply.store(true, Ordering::SeqCst);

    // Replica A wins the claim and re-sends; its reply is lost again.
    core.pending_redrives.lock().map_err(|_| "poisoned lock")?.clear();
    core.redrive_pending(&aged).await;
    assert_eq!(bus.sends.load(Ordering::SeqCst), 2);

    // Replica B holds the same stale view and nothing in memory.
    core.pending_redrives.lock().map_err(|_| "poisoned lock")?.clear();
    core.redrive_pending(&aged).await;
    core.redrive_pending(&pending).await;
    assert_eq!(
        bus.sends.load(Ordering::SeqCst),
        2,
        "a claimed re-send is not repeated by another replica"
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_redrive_stops_redriving_and_starts_cleanup() -> TestResult {
    let (mut core, _bus, _drive, request) = native_failure_fixture(None).await?;
    let bus = Arc::new(RefusedRedriveBus {
        dispatches: AtomicU32::new(0),
    });
    core.bus = bus.clone();
    core.config.provider_dispatch_timeout = Duration::ZERO;

    let first = core.handle_lease_request(request).await;
    assert!(matches!(first, LeaseResponse::Denied(_)), "{first:?}");
    let pending = core.active_leases.all().await?.pop().ok_or("no lease record")?;

    core.reconcile_expired_leases().await;

    assert_eq!(bus.dispatches.load(Ordering::SeqCst), 2);
    assert!(core.pending_redrives.lock().map_err(|_| "poisoned lock")?.is_empty());
    let record = core
        .active_leases
        .get(&pending.lease_id)
        .await?
        .ok_or("record missing")?;
    assert_eq!(
        record.status,
        LeaseStatus::Cancelling,
        "a refused attempt waits for the worker to confirm cleanup"
    );
    assert!(
        record
            .dispatch_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.pending_dispatch.is_none()),
        "a refused attempt drops its journaled dispatch"
    );

    core.reconcile_expired_leases().await;
    assert_eq!(
        bus.dispatches.load(Ordering::SeqCst),
        2,
        "a refused attempt is never re-sent"
    );
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod cleanup_receipts;

#[tokio::test]
async fn targeted_retry_preserves_extended_deadline() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    core.bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let request = make_request(vec![ProviderKind::Fly])?;
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request.clone()).await else {
        return Err("fixture denied".into());
    };
    let response = core
        .handle_extend_lease_timeout(ExtendLeaseTimeoutRequest {
            lease_id: granted.lease_id.clone(),
            additional_seconds: 30,
        })
        .await;
    let ExtendLeaseTimeoutResponse::Extended { new_deadline } = response else {
        return Err(format!("{response:?}").into());
    };
    let LeaseResponse::Granted(retried) = core.handle_lease_request(request).await else {
        return Err("retry denied".into());
    };
    assert_eq!(
        retried.deadline, new_deadline,
        "idempotent retries must preserve durable lease mutations"
    );
    Ok(())
}

#[tokio::test]
async fn an_untargeted_grant_keeps_the_granting_worker_for_the_release_event() -> TestResult {
    let worker = trogon_sandbox_contract::WorkerId::new("01JWORKER")?;
    let reply = Bytes::from(serde_json::to_vec(&CoreProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new("sandbox-abc")?,
        evidence_subject: "sandbox.evidence.abc.>".to_string(),
        worker_id: Some(worker.clone()),
        attempt_id: None,
    })?);
    let mut core = make_core(reply.clone())?;
    let bus = Arc::new(CapturingBus::new(reply));
    core.bus = bus.clone();
    let LeaseResponse::Granted(granted) = core.handle_lease_request(make_request(vec![ProviderKind::Mock])?).await
    else {
        return Err("fixture denied".into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("record missing")?;
    assert_eq!(record.worker_id, Some(worker.clone()));

    core.handle_cancel(granted.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;

    let event: LeaseReleased = {
        let published = bus.published.lock().map_err(|_| "poisoned lock")?;
        serde_json::from_slice(
            &published
                .iter()
                .find(|(subject, _)| subject == &subjects::lease_released(&granted.lease_id))
                .ok_or("no release event")?
                .1,
        )?
    };
    assert_eq!(event.worker_id, Some(worker.clone()));
    Ok(())
}

#[tokio::test]
async fn a_targeted_lease_reaped_at_its_deadline_forgets_its_launch_keys() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    core.bus = bus.clone();
    let LeaseResponse::Granted(granted) = core.handle_lease_request(make_request(vec![ProviderKind::Fly])?).await
    else {
        return Err("fixture denied".into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("record missing")?;
    assert_eq!(
        core.command_launches
            .claim(&granted.lease_id, &LaunchKey::new("k")?, "f")
            .await?,
        Claim::Claimed
    );
    let mut expired = record.clone();
    expired.deadline = SystemTime::UNIX_EPOCH;
    let attempt = record.dispatch_attempt.as_ref().ok_or("missing value")?;
    core.idempotency
        .replace_attempt(&record.tenant, &attempt.idempotency_key, &record, &expired)
        .await?;
    core.active_leases.put(&expired).await?;
    core.reconcile_expired_leases().await;

    assert_eq!(
        core.command_launches
            .claim(&granted.lease_id, &LaunchKey::new("k")?, "f")
            .await?,
        Claim::Claimed
    );
    Ok(())
}

#[tokio::test]
async fn targeted_expiry_emits_snapshot_plan_with_stable_worker_and_expired_reason() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    core.bus = bus.clone();
    let policy = trogon_sandbox_contract::SnapshotPolicy {
        artifact_type: ArtifactType::SandboxRootfsV1,
        target_repository: "snapshots/test-tenant".into(),
        retention: None,
        prewarm: None,
        tag_name: None,
    };
    let request = edit_request_spec(make_request(vec![ProviderKind::Fly])?, |parts| {
        parts.snapshot_out = Some(policy.clone());
    })?;
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request).await else {
        return Err("fixture denied".into());
    };
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .ok_or("record missing")?;
    let mut expired = record.clone();
    expired.deadline = SystemTime::UNIX_EPOCH;
    let attempt = record.dispatch_attempt.as_ref().ok_or("missing value")?;
    core.idempotency
        .replace_attempt(&record.tenant, &attempt.idempotency_key, &record, &expired)
        .await?;
    core.active_leases.put(&expired).await?;
    core.reconcile_expired_leases().await;
    let event: LeaseReleased = {
        let published = bus.published.lock().map_err(|_| "poisoned lock")?;
        serde_json::from_slice(
            &published
                .iter()
                .find(|(subject, _)| subject == &subjects::lease_released(&granted.lease_id))
                .ok_or("no release event")?
                .1,
        )?
    };
    assert_eq!(event.reason, LeaseReleasedReason::Expired);
    assert_eq!(event.snapshot_out, Some(policy));
    assert_eq!(event.worker_id, record.worker_id);
    assert_eq!(core.quota.active_lease_count(&record.tenant).await?, 0);
    Ok(())
}

#[tokio::test]
async fn legacy_cache_key_collision_cannot_expose_another_tenants_modern_grant() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    core.bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let mut owner = make_request(vec![ProviderKind::Fly])?;
    owner.tenant = TenantId::new("acme")?;
    owner.idempotency_key = LeaseRequestKey::new("team_key")?;
    let LeaseResponse::Granted(victim) = core.handle_lease_request(owner.clone()).await else {
        return Err("fixture denied".into());
    };
    core.idempotency
        .put_if_absent(&owner.tenant, owner.idempotency_key.as_str(), &victim)
        .await?;
    let mut attacker = make_request(vec![ProviderKind::Fly])?;
    attacker.tenant = TenantId::new("acme_team")?;
    attacker.idempotency_key = LeaseRequestKey::new("key")?;
    let response = core.handle_lease_request(attacker).await;
    assert!(
        !matches!(response, LeaseResponse::Granted(ref grant) if grant.lease_id == victim.lease_id),
        "legacy cache aliases cannot authorize a foreign grant: {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn named_index_key_collision_cannot_reconnect_another_tenants_running_lease() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let mut core = make_core(mock_provider_response()?)?;
    core.named_leases = Arc::new(
        crate::named_lease::NatsKvNamedLeaseStore::new(async_nats::jetstream::new(server.client.clone())).await?,
    );
    let owner = TenantId::new("acme")?;
    let victim = LeaseId::new("foreign-running-lease");
    core.active_leases.put(&running_source_record(&owner, &victim)?).await?;
    core.named_leases.set(&owner, "team_key", &victim).await?;
    let mut attacker = make_request(vec![ProviderKind::Mock])?;
    attacker.tenant = TenantId::new("acme_team")?;
    attacker.name = Some(LeaseName::new("key")?);
    attacker.persistent = LeasePersistence::from(false);
    let response = core.handle_lease_request(attacker).await;
    assert!(
        !matches!(response, LeaseResponse::Granted(ref grant) if grant.lease_id == victim),
        "named index aliases cannot authorize a foreign reconnect: {response:?}"
    );
    let lookup = core
        .handle_get_lease_by_name(GetLeaseByNameRequest {
            tenant: TenantId::new("acme_team")?,
            name: LeaseName::new("key")?,
        })
        .await;
    assert!(
        !matches!(lookup, GetLeaseByNameResponse::Found { ref row } if row.lease_id == victim),
        "named index aliases cannot disclose a foreign record: {lookup:?}"
    );
    Ok(())
}

#[tokio::test]
async fn targeted_purge_does_not_request_a_new_resumable_snapshot() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    core.bus = bus.clone();
    let request = edit_request_spec(make_request(vec![ProviderKind::Fly])?, |parts| {
        parts.snapshot_out = Some(trogon_sandbox_contract::SnapshotPolicy {
            artifact_type: ArtifactType::SandboxRootfsV1,
            target_repository: "snapshots/test-tenant".into(),
            retention: None,
            prewarm: None,
            tag_name: None,
        });
    })?;
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request).await else {
        return Err("fixture denied".into());
    };
    assert!(matches!(
        core.handle_delete_lease(DeleteLeaseRequest {
            tenant,
            lease_id: granted.lease_id.clone(),
            operation_id: None
        })
        .await,
        DeleteLeaseResponse::Deleted { operation_id: None }
    ));
    let published = bus.published.lock().map_err(|_| "poisoned lock")?;
    let event: LeaseReleased = serde_json::from_slice(
        &published
            .iter()
            .find(|(subject, _)| subject == &subjects::lease_released(&granted.lease_id))
            .ok_or("no release event")?
            .1,
    )?;
    assert!(
        event.snapshot_out.is_none(),
        "purging a sandbox must not enqueue another resumable snapshot"
    );
    Ok(())
}
