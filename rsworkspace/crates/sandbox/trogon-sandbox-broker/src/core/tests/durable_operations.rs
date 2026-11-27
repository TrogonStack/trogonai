use super::*;

use crate::operations::{Entry, InMemoryOperationStore, NatsKvOperationStore, OperationStore};
use crate::quota::NatsKvQuotaStore;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod quota_recovery;
use quota_recovery::{durable_quota_values, interrupt_quota_drive, reconcile_positive_legacy_receipt};

#[tokio::test]
async fn a_pause_that_lost_its_reply_finishes_after_a_restart() -> TestResult {
    let bus = FreezeWorkerBus::new(true);
    let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let store: Arc<dyn crate::operations::OperationStore> = Arc::new(crate::operations::InMemoryOperationStore::new());
    let tenant = TenantId::new("acme")?;
    let first = core_over(bus.clone(), active.clone(), store.clone(), "broker-a")?;
    let lease_id = running_lease_in(&first, &tenant, "crash-lease").await?;

    // The worker froze the sandbox, then the broker died before it could answer.
    let attempt = tokio::time::timeout(
        Duration::from_millis(300),
        first.handle_pause_lease(pause_with(&tenant, &lease_id, "crash-op", 600)?),
    )
    .await;
    assert!(attempt.is_err(), "the first broker never answered");
    drop(first);
    assert_eq!(bus.effective.load(Ordering::SeqCst), 1);
    let stuck = store_record(&store, &tenant, "crash-op").await?;
    assert_eq!(stuck.state, OperationState::Running);
    assert_eq!(
        active.get(&lease_id).await?.ok_or("record missing")?.status,
        LeaseStatus::Running
    );

    let second = core_over(bus.clone(), active.clone(), store.clone(), "broker-b")?;
    assert_eq!(second.recover_operations().await, 1);

    let done = store_record(&store, &tenant, "crash-op").await?;
    assert_eq!(done.state, OperationState::Succeeded);
    assert!(matches!(done.result, Some(OperationResult::Paused { .. })));
    assert_eq!(
        active.get(&lease_id).await?.ok_or("record missing")?.status,
        LeaseStatus::Paused
    );
    assert_eq!(bus.effective.load(Ordering::SeqCst), 1, "one effective freeze");
    assert_eq!(bus.calls.load(Ordering::SeqCst), 2, "the worker saw the repeat");

    // The client retries with the same id and gets the stored outcome, no new worker call.
    let PauseLeaseResponse::Paused { operation_id, .. } = second
        .handle_pause_lease(pause_with(&tenant, &lease_id, "crash-op", 600)?)
        .await
    else {
        return Err("the retry replays the stored outcome".into());
    };
    assert_eq!(operation_id, Some(op_id("crash-op")?));
    assert_eq!(bus.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn a_repeated_operation_id_replays_without_a_second_worker_call() -> TestResult {
    let bus = FreezeWorkerBus::new(false);
    let core = fresh_core(bus.clone())?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "replay-lease").await?;

    let first = core
        .handle_pause_lease(pause_with(&tenant, &lease_id, "op-1", 600)?)
        .await;
    let again = core
        .handle_pause_lease(pause_with(&tenant, &lease_id, "op-1", 600)?)
        .await;
    assert!(matches!(
        first,
        PauseLeaseResponse::Paused {
            operation_id: Some(_),
            ..
        }
    ));
    assert_eq!(first, again);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn an_operation_id_reused_for_a_different_request_is_a_conflict() -> TestResult {
    let bus = FreezeWorkerBus::new(false);
    let core = fresh_core(bus.clone())?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "conflict-lease").await?;
    core.handle_pause_lease(pause_with(&tenant, &lease_id, "op-1", 600)?)
        .await;

    let different = core
        .handle_pause_lease(pause_with(&tenant, &lease_id, "op-1", 60)?)
        .await;
    assert!(matches!(
        different,
        PauseLeaseResponse::Denied(LeaseError::OperationConflict { .. })
    ));
    let other_verb = core
        .handle_resume_lease(ResumeLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
            operation_id: Some(op_id("op-1")?),
        })
        .await;
    assert!(matches!(
        other_verb,
        ResumeLeaseResponse::Denied(LeaseError::OperationConflict { .. })
    ));
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_operation_replays_its_error() -> TestResult {
    let core = fresh_core(FreezeWorkerBus::new(false))?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("failed-lease");
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.status = LeaseStatus::Pending;
    core.active_leases.put(&record).await?;
    // Pausing a lease that is not running is refused; the refusal is the operation's outcome.
    let request = pause_with(&tenant, &lease_id, "op-fail", 600)?;
    let first = core.handle_pause_lease(request.clone()).await;
    assert!(matches!(
        first,
        PauseLeaseResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    assert_eq!(core.handle_pause_lease(request).await, first);
    let GetOperationResponse::Found { operation } = core
        .handle_get_operation(GetOperationRequest {
            tenant,
            operation_id: op_id("op-fail")?,
        })
        .await
    else {
        return Err("recorded".into());
    };
    assert_eq!(operation.state, OperationState::Failed);
    Ok(())
}

#[tokio::test]
async fn get_operation_is_tenant_scoped() -> TestResult {
    let core = fresh_core(FreezeWorkerBus::new(false))?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "scoped-lease").await?;
    core.handle_pause_lease(pause_with(&tenant, &lease_id, "op-1", 600)?)
        .await;

    let mine = core
        .handle_get_operation(GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: op_id("op-1")?,
        })
        .await;
    assert!(matches!(mine, GetOperationResponse::Found { ref operation }
        if operation.state == OperationState::Succeeded));
    let foreign = core
        .handle_get_operation(GetOperationRequest {
            tenant: TenantId::new("rival")?,
            operation_id: op_id("op-1")?,
        })
        .await;
    assert_eq!(foreign, GetOperationResponse::NotFound);
    let missing = core
        .handle_get_operation(GetOperationRequest {
            tenant,
            operation_id: op_id("never-made")?,
        })
        .await;
    assert_eq!(missing, GetOperationResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_delete_operation_replays_after_the_lease_is_gone() -> TestResult {
    let core = fresh_core(FreezeWorkerBus::new(false))?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "delete-lease").await?;
    let request = DeleteLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        operation_id: Some(op_id("del-1")?),
    };
    let first = core.handle_delete_lease(request.clone()).await;
    assert_eq!(
        first,
        DeleteLeaseResponse::Deleted {
            operation_id: Some(op_id("del-1")?)
        }
    );
    assert!(core.active_leases.get(&lease_id).await?.is_none());
    assert_eq!(core.handle_delete_lease(request).await, first);
    Ok(())
}

#[tokio::test]
async fn without_an_operation_store_lifecycle_verbs_run_inline() -> TestResult {
    let core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "inline-lease").await?;
    let response = core
        .handle_pause_lease(pause_with(&tenant, &lease_id, "ignored", 600)?)
        .await;
    assert!(matches!(
        response,
        PauseLeaseResponse::Paused { operation_id: None, .. }
    ));
    assert_eq!(
        core.handle_get_operation(GetOperationRequest {
            tenant,
            operation_id: op_id("ignored")?,
        })
        .await,
        GetOperationResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn a_targeted_create_that_lost_its_reply_runs_once_after_a_restart() -> TestResult {
    let bus = LaunchBus::new(true);
    let store = op_store();
    let mut first = durable_core(bus.clone(), store.clone(), "broker-a")?;
    use_targeted_worker(&mut first, ProviderKind::Fly).await?;
    let tenant = TenantId::new("test-tenant")?;

    crash_mid_create(&first, create_with(ProviderKind::Fly, "crash-create")?).await;
    assert_eq!(bus.launches()?, 1);
    let stuck = store_record(&store, &tenant, "crash-create").await?;
    assert_eq!(stuck.state, OperationState::Running);
    assert_eq!(stuck.kind, OperationKind::Create);
    let Some(OperationRequest::Create { dispatch, .. }) = stuck.request else {
        return Err("the request is kept while the create is open".into());
    };
    assert!(dispatch.is_some(), "the payload was stored before the send");
    let pending = first.active_leases.all().await?;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, LeaseStatus::Pending);

    let second = restarted(&first, bus.clone(), store.clone())?;
    drop(first);
    assert_eq!(second.recover_operations().await, 1);

    let done = store_record(&store, &tenant, "crash-create").await?;
    assert_eq!(done.state, OperationState::Succeeded);
    assert!(done.request.is_none(), "a finished create drops its request");
    let Some(OperationResult::Granted(grant)) = done.result else {
        return Err("the create ends granted".into());
    };
    let record = second
        .active_leases
        .get(&grant.lease_id)
        .await?
        .ok_or("record missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    assert_eq!(bus.launches()?, 1, "exactly one launch");
    assert_eq!(bus.calls.load(Ordering::SeqCst), 2, "the worker saw the repeat");

    // The client retries with the same id and gets the same grant, no worker call.
    let LeaseResponse::Granted(replayed) = second
        .handle_lease_request(create_with(ProviderKind::Fly, "crash-create")?)
        .await
    else {
        return Err("the retry replays the grant".into());
    };
    assert_eq!(replayed.lease_id, grant.lease_id);
    assert!(!replayed.created.is_created());
    assert_eq!(replayed.operation_id, Some(op_id("crash-create")?));
    assert_eq!(bus.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn a_legacy_create_that_lost_its_reply_waits_for_positive_reconciliation() -> TestResult {
    let bus = LaunchBus::new(true);
    let store = op_store();
    let first = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let tenant = TenantId::new("test-tenant")?;

    crash_mid_create(&first, create_with(ProviderKind::Mock, "legacy-crash")?).await;
    assert_eq!(bus.launches()?, 1);

    let second = restarted(&first, bus.clone(), store.clone())?;
    drop(first);
    assert_eq!(second.recover_operations().await, 0);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    assert_eq!(quota_usage(&second, &tenant).await?, (60, 1));
    assert!(!store_record(&store, &tenant, "legacy-crash").await?.state.is_terminal());
    reconcile_positive_legacy_receipt(&second, &bus, &store, &tenant, "legacy-crash").await?;
    assert_eq!(second.recover_operations().await, 1);

    let done = store_record(&store, &tenant, "legacy-crash").await?;
    assert_eq!(done.state, OperationState::Succeeded);
    let Some(OperationResult::Granted(grant)) = done.result else {
        return Err("the create ends granted".into());
    };
    assert_eq!(
        second
            .active_leases
            .get(&grant.lease_id)
            .await?
            .ok_or("record missing")?
            .status,
        LeaseStatus::Running
    );
    let ids = bus.lease_ids.lock().map_err(|_| "poisoned lock")?.clone();
    assert_eq!(ids.len(), 1, "positive reconciliation must not send a second dispatch");
    assert_eq!(ids[0], grant.lease_id.as_str());
    assert_eq!(bus.launches()?, 1, "exactly one launch");
    Ok(())
}

struct LegacyFleetReplyLossBus {
    guards: [trogon_sandbox_protocol::RedispatchGuard; 2],
    allocations: std::sync::Mutex<Vec<(usize, LeaseId, trogon_sandbox_contract::SandboxId)>>,
    calls: AtomicU32,
    allocated: tokio::sync::Notify,
}

impl LegacyFleetReplyLossBus {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            guards: Default::default(),
            allocations: Default::default(),
            calls: AtomicU32::new(0),
            allocated: tokio::sync::Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for LegacyFleetReplyLossBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        assert_eq!(subject, "sandbox.provider.mock.lease");
        let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
        assert!(dispatch.target.is_none());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let worker = call as usize % self.guards.len();
        let grant = self.guards[worker]
            .get_or_launch(&dispatch.lease_id, || async {
                let sandbox_id = trogon_sandbox_contract::SandboxId::new(format!("replica-{worker}-{call}"))?;
                self.allocations
                    .lock()
                    .map_err(|_| anyhow::anyhow!("poisoned lock"))?
                    .push((worker, dispatch.lease_id.clone(), sandbox_id.clone()));
                Ok::<_, anyhow::Error>(CoreProviderLease {
                    epoch: None,
                    sandbox_id,
                    evidence_subject: subjects::evidence_subject(&dispatch.lease_id),
                    worker_id: Some(trogon_sandbox_contract::WorkerId::new(format!(
                        "legacy-replica-{worker}"
                    ))?),
                    attempt_id: None,
                })
            })
            .await?;
        if call == 0 {
            self.allocated.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(serde_json::to_vec(&grant)?.into())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_legacy_create_lost_reply_does_not_launch_on_another_replica_after_restart() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(server.client.clone());
    let operation = "legacy-fleet-reply-loss";
    let tenant = TenantId::new("test-tenant")?;
    let bus = LegacyFleetReplyLossBus::new();
    let store: Arc<dyn OperationStore> =
        Arc::new(NatsKvOperationStore::new(context.clone(), "legacy-fleet-operations", 1).await?);
    let mut first = durable_core(bus.clone(), store.clone(), "legacy-broker-a")?;
    first.quota = Arc::new(NatsKvQuotaStore::new(context.clone(), "legacy-fleet-quota").await?);
    let first = Arc::new(first);
    let creating = first.clone();
    let create_request = create_with(ProviderKind::Mock, operation)?;
    let interrupted = interrupt_quota_drive(&bus.allocated, async move {
        creating.handle_lease_request(create_request).await
    })
    .await;
    assert!(interrupted?.is_none(), "the first allocation reply was lost");
    assert_eq!(bus.allocations.lock().map_err(|_| "poisoned lock")?.len(), 1);
    assert_eq!(durable_quota_values(&first, &tenant).await?, (60, 300, 1));
    let before = store_record(&store, &tenant, operation).await?;
    assert_eq!(before.state, OperationState::Running);
    let Some(OperationRequest::Create {
        quota:
            Some(trogon_sandbox_contract::CreateQuotaStep::Reserved {
                reservation: Some(reservation),
                ..
            }),
        ..
    }) = &before.request
    else {
        return Err("the first dispatch retains its exact quota receipt".into());
    };
    let reservation = reservation.clone();
    assert!(
        first
            .active_leases
            .get(before.target.as_ref().ok_or("the running operation has a target")?)
            .await?
            .is_none()
    );

    let reopened: Arc<dyn OperationStore> =
        Arc::new(NatsKvOperationStore::new(context.clone(), "legacy-fleet-operations", 1).await?);
    let mut second = restarted(&first, bus.clone(), reopened.clone())?;
    second.quota = Arc::new(NatsKvQuotaStore::new(context, "legacy-fleet-quota").await?);
    drop(first);
    second.recover_operations().await;
    let quota = durable_quota_values(&second, &tenant).await?;
    let GetOperationResponse::Found { operation: observed } = second
        .handle_get_operation(GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: op_id(operation)?,
        })
        .await
    else {
        return Err("the ambiguous create must remain observable".into());
    };
    let allocations = bus.allocations.lock().map_err(|_| "poisoned lock")?.clone();
    let retained_reservation = matches!(
        &observed.request,
        Some(OperationRequest::Create {
            quota: Some(trogon_sandbox_contract::CreateQuotaStep::Reserved {
                reservation: Some(kept),
                ..
            }),
            ..
        }) if kept == &reservation
    );
    assert!(
        allocations.len() == 1
            && bus.calls.load(Ordering::SeqCst) == 1
            && !observed.state.is_terminal()
            && observed.request.is_some()
            && retained_reservation
            && observed.target == before.target
            && quota == (60, 300, 1),
        "ambiguous legacy create replayed without fleet ownership: allocations={allocations:?}, dispatches={}, state={:?}, request_retained={}, exact_reservation_retained={retained_reservation}, quota={quota:?}",
        bus.calls.load(Ordering::SeqCst),
        observed.state,
        observed.request.is_some()
    );
    Ok(())
}

#[tokio::test]
async fn a_legacy_create_that_crashed_after_reserving_quota_does_not_reserve_twice() -> TestResult {
    let bus = LaunchBus::new(true);
    let store = op_store();
    let first = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let tenant = TenantId::new("test-tenant")?;

    crash_mid_create(&first, create_with(ProviderKind::Mock, "reserve-crash")?).await;
    assert_eq!(
        quota_usage(&first, &tenant).await?,
        (60, 1),
        "atomic reservation holds budget and active slot"
    );

    let second = restarted(&first, bus.clone(), store.clone())?;
    drop(first);
    assert_eq!(second.recover_operations().await, 0);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    assert_eq!(quota_usage(&second, &tenant).await?, (60, 1));
    reconcile_positive_legacy_receipt(&second, &bus, &store, &tenant, "reserve-crash").await?;
    assert_eq!(second.recover_operations().await, 1);

    let done = store_record(&store, &tenant, "reserve-crash").await?;
    assert_eq!(done.state, OperationState::Succeeded);
    assert_eq!(
        quota_usage(&second, &tenant).await?,
        (60, 1),
        "one lease holds one reservation and one slot"
    );
    Ok(())
}

#[tokio::test]
async fn a_legacy_create_without_a_quota_receipt_requires_manual_reconciliation() -> TestResult {
    let bus = LaunchBus::new(false);
    let store = op_store();
    let first = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let tenant = TenantId::new("test-tenant")?;

    // The lease reached the active-lease store, then the broker died
    // before it was counted against the tenant.
    let LeaseResponse::Granted(grant) = first
        .handle_lease_request(make_request(vec![ProviderKind::Mock])?)
        .await
    else {
        return Err("the plain lease is granted".into());
    };
    first.quota.decrement_active_leases(&tenant).await?;
    assert_eq!(quota_usage(&first, &tenant).await?, (60, 0));

    let old = crate::operations::Operations::new(store.clone(), "broker-a");
    let request = OperationRequest::Create {
        request: Box::new(make_request(vec![ProviderKind::Mock])?),
        attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("attempt-count")?,
        dispatch: None,
        quota: None,
    };
    old.begin(
        &tenant,
        &op_id("count-crash")?,
        &grant.lease_id,
        request,
        SystemTime::now(),
    )
    .await?;
    let (_, token) = old
        .claim(&tenant, &op_id("count-crash")?, SystemTime::now())
        .await?
        .ok_or("record missing")?;
    assert!(
        old.save_quota_step(
            &token,
            Some(trogon_sandbox_contract::CreateQuotaStep::Reserved {
                cpu_seconds: 60,
                wall_seconds: 300,
                reservation: None,
            }),
        )
        .await?
    );

    let second = restarted(&first, bus.clone(), store.clone())?;
    drop(first);
    assert_eq!(second.recover_operations().await, 0);

    let done = store_record(&store, &tenant, "count-crash").await?;
    assert_eq!(done.state, OperationState::Queued);
    assert!(
        done.progress
            .as_ref()
            .is_some_and(|message| message.contains("manual accounting reconciliation"))
    );
    assert_eq!(quota_usage(&second, &tenant).await?, (60, 0));
    assert_eq!(bus.launches()?, 1, "the recorded lease is not launched again");
    Ok(())
}

#[tokio::test]
async fn a_denied_durable_create_leaves_the_quota_untouched() -> TestResult {
    let bus = LaunchBus::new(false);
    let store = op_store();
    let core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let tenant = TenantId::new("test-tenant")?;
    let mut request = create_with(ProviderKind::Mock, "denied-create")?;
    request = edit_request_spec(request, |parts| parts.memory_mib = NonZeroU32::MAX)?;
    assert!(matches!(
        core.handle_lease_request(request).await,
        LeaseResponse::Denied(_)
    ));
    assert!(
        core.quota
            .quota_record(&tenant)
            .await?
            .is_none_or(|q| q.cpu_seconds_used == 0 && q.active_lease_count() == 0)
    );
    Ok(())
}

#[tokio::test]
async fn a_named_create_is_not_claimed_twice_across_a_redrive() -> TestResult {
    let bus = LaunchBus::new(true);
    let store = op_store();
    let first = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let tenant = TenantId::new("test-tenant")?;
    let mut request = create_with(ProviderKind::Mock, "named-crash")?;
    request.name = Some(LeaseName::new("shared-box")?);

    crash_mid_create(&first, request.clone()).await;
    let second = restarted(&first, bus.clone(), store.clone())?;
    drop(first);
    assert_eq!(second.recover_operations().await, 0);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    reconcile_positive_legacy_receipt(&second, &bus, &store, &tenant, "named-crash").await?;
    assert_eq!(second.recover_operations().await, 1);
    let done = store_record(&store, &tenant, "named-crash").await?;
    assert_eq!(done.state, OperationState::Succeeded);
    let Some(OperationResult::Granted(grant)) = done.result else {
        return Err("the create ends granted".into());
    };

    let mut other = create_with(ProviderKind::Mock, "named-other")?;
    other.name = Some(LeaseName::new("shared-box")?);
    other.idempotency_key = LeaseRequestKey::new("another-key")?;
    let LeaseResponse::Granted(existing) = second.handle_lease_request(other).await else {
        return Err("the name resolves to the running lease".into());
    };
    assert_eq!(existing.lease_id, grant.lease_id);
    assert!(!existing.created.is_created());
    assert_eq!(bus.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn a_repeated_create_operation_id_replays_without_a_second_launch() -> TestResult {
    let bus = LaunchBus::new(false);
    let core = durable_core(bus.clone(), op_store(), "broker-a")?;

    let LeaseResponse::Granted(first) = core
        .handle_lease_request(create_with(ProviderKind::Mock, "create-1")?)
        .await
    else {
        return Err("the create is granted".into());
    };
    assert!(first.created.is_created());
    let LeaseResponse::Granted(again) = core
        .handle_lease_request(create_with(ProviderKind::Mock, "create-1")?)
        .await
    else {
        return Err("the repeat replays".into());
    };
    assert_eq!(again.lease_id, first.lease_id);
    assert!(!again.created.is_created());
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_create_operation_id_reused_for_a_different_request_is_a_conflict() -> TestResult {
    let bus = LaunchBus::new(false);
    let core = durable_core(bus.clone(), op_store(), "broker-a")?;
    core.handle_lease_request(create_with(ProviderKind::Mock, "create-1")?)
        .await;

    let mut different = create_with(ProviderKind::Mock, "create-1")?;
    different.ttl_seconds = 120;
    assert!(matches!(
        core.handle_lease_request(different).await,
        LeaseResponse::Denied(LeaseError::OperationConflict { .. })
    ));
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_restore_is_recorded_as_a_restore_operation() -> TestResult {
    let bus = LaunchBus::new(false);
    let store = op_store();
    let core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let mut request = make_snapshot_request(vec![ProviderKind::Mock])?;
    request.operation_id = Some(op_id("restore-1")?);
    let response = core.handle_lease_request(request).await;
    assert!(matches!(response, LeaseResponse::Granted(_)), "{response:?}");
    let record = store_record(&store, &TenantId::new("test-tenant")?, "restore-1").await?;
    assert_eq!(record.kind, OperationKind::Restore);
    assert_eq!(record.state, OperationState::Succeeded);
    Ok(())
}

#[tokio::test]
async fn a_create_without_an_operation_id_leaves_no_record() -> TestResult {
    let bus = LaunchBus::new(false);
    let store = op_store();
    let core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let response = core.handle_lease_request(make_request(vec![ProviderKind::Mock])?).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    let operations = crate::operations::Operations::new(store, "reader");
    assert!(operations.list_active().await?.is_empty());
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}
