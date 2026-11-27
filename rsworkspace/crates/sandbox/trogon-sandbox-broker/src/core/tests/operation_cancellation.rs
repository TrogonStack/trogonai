use super::*;
use crate::operations::{Operations, Settled};
use std::sync::atomic::AtomicBool;
use trogon_sandbox_contract::{
    CancelOperationRequest, CancelOperationResponse, CreatedLeaseCompensation, OperationCapabilitiesRequest,
};

struct CancelableLaunchBus {
    inner: Arc<LaunchBus>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    cleanup_entered: tokio::sync::Notify,
    cleaned: std::sync::Mutex<Vec<trogon_sandbox_contract::LeaseAttemptIdentity>>,
    owned_cleaned: std::sync::Mutex<Vec<trogon_sandbox_contract::CancelOwnedLeaseRequest>>,
    cleanup_fails: AtomicBool,
}

impl CancelableLaunchBus {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: LaunchBus::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            cleanup_entered: tokio::sync::Notify::new(),
            cleaned: Default::default(),
            owned_cleaned: Default::default(),
            cleanup_fails: AtomicBool::new(false),
        })
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for CancelableLaunchBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if let Ok(cleanup) = serde_json::from_slice::<trogon_sandbox_contract::CancelOwnedLeaseRequest>(&payload) {
            let receipt = locked(&self.inner.receipts)?
                .get(cleanup.lease_id.as_str())
                .cloned()
                .context("the worker knows this exact lease")?;
            assert_eq!(receipt.sandbox_id, cleanup.sandbox_id);
            assert_eq!(
                subject,
                trogon_sandbox_protocol::subjects::provider_cancel_owned(ProviderKind::E2b, &cleanup.lease_id)
            );
            locked(&self.owned_cleaned)?.push(cleanup.clone());
            self.cleanup_entered.notify_one();
            anyhow::ensure!(
                !self.cleanup_fails.load(Ordering::SeqCst),
                "owned cleanup acknowledgement was lost"
            );
            return Ok(
                serde_json::to_vec(&trogon_sandbox_contract::CancelOwnedLeaseResponse::Cleaned {
                    tenant: cleanup.tenant,
                    lease_id: cleanup.lease_id,
                    sandbox_id: cleanup.sandbox_id,
                })?
                .into(),
            );
        }
        if let Ok(cleanup) = serde_json::from_slice::<trogon_sandbox_contract::ProviderCleanupRequest>(&payload) {
            locked(&self.cleaned)?.push(cleanup.identity.clone());
            self.cleanup_entered.notify_one();
            anyhow::ensure!(
                !self.cleanup_fails.load(Ordering::SeqCst),
                "exact cleanup acknowledgement was lost"
            );
            return Ok(
                serde_json::to_vec(&trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
                    identity: cleanup.identity,
                    intent: cleanup.intent,
                })?
                .into(),
            );
        }
        let reply = self.inner.request(subject, payload).await?;
        self.entered.notify_one();
        self.release.notified().await;
        Ok(reply)
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.inner.publish(subject, payload).await
    }
}

fn cancel(tenant: &TenantId, id: &str) -> anyhow::Result<CancelOperationRequest> {
    Ok(CancelOperationRequest {
        tenant: tenant.clone(),
        operation_id: op_id(id)?,
        compensate_created: CreatedLeaseCompensation::from(false),
    })
}

fn compensate(tenant: &TenantId, id: &str) -> anyhow::Result<CancelOperationRequest> {
    Ok(CancelOperationRequest {
        compensate_created: CreatedLeaseCompensation::from(true),
        ..cancel(tenant, id)?
    })
}

#[tokio::test]
async fn operation_cancel_before_create_arrival_is_durable_and_prevents_dispatch() -> TestResult {
    let bus = LaunchBus::new(false);
    let store = op_store();
    let core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let request = create_with(ProviderKind::Mock, "canceled-first")?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&request.tenant, "canceled-first")?)
            .await,
        CancelOperationResponse::Pending
    );
    let reopened = restarted(&core, bus.clone(), store.clone())?;
    assert!(matches!(
        reopened.handle_lease_request(request.clone()).await,
        LeaseResponse::Denied(LeaseError::OperationCanceled { .. })
    ));
    assert_eq!(bus.launches()?, 0);
    assert_eq!(
        store_record(&store, &request.tenant, "canceled-first").await?.state,
        OperationState::Canceled
    );
    assert_eq!(
        reopened
            .handle_cancel_operation(cancel(&request.tenant, "canceled-first")?)
            .await,
        CancelOperationResponse::Canceled
    );
    Ok(())
}

#[tokio::test]
async fn operation_cancel_after_dispatch_cleans_only_the_exact_owned_late_grant() -> TestResult {
    let bus = CancelableLaunchBus::new();
    let store = op_store();
    let mut core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    use_targeted_worker(&mut core, ProviderKind::Fly).await?;
    let request = create_with(ProviderKind::Fly, "cancel-late-grant")?;
    let tenant = request.tenant.clone();
    let unrelated = running_source_record(&tenant, &LeaseId::new("unrelated-running"))?;
    core.active_leases.put(&unrelated).await?;
    let core = Arc::new(core);
    let running = core.clone();
    let create = tokio::spawn(async move { running.handle_lease_request(request).await });
    tokio::time::timeout(Duration::from_secs(3), bus.entered.notified()).await?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "cancel-late-grant")?)
            .await,
        CancelOperationResponse::Pending
    );
    bus.release.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(5), create).await??;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::OperationCanceled { .. })
    ));
    let operation = store_record(&store, &tenant, "cancel-late-grant").await?;
    assert_eq!(operation.state, OperationState::Canceled);
    {
        let cleaned = locked(&bus.cleaned)?;
        assert_eq!(cleaned.len(), 1);
        assert_eq!(operation.target.as_ref(), Some(&cleaned[0].lease_id));
        assert_eq!(cleaned[0].provider, ProviderKind::Fly);
    }
    assert_eq!(
        core.active_leases
            .get(&unrelated.lease_id)
            .await?
            .ok_or("the value is missing")?,
        unrelated
    );
    Ok(())
}

#[tokio::test]
async fn operation_cancel_never_deletes_an_adopted_named_lease() -> TestResult {
    let bus = LaunchBus::new(false);
    let core = durable_core(bus.clone(), op_store(), "broker-a")?;
    let mut existing = create_with(ProviderKind::Mock, "original-owner")?;
    existing.name = Some(LeaseName::new("shared-name")?);
    let LeaseResponse::Granted(original) = core.handle_lease_request(existing).await else {
        return Err("initial owned lease".into());
    };
    let mut adopt = create_with(ProviderKind::Mock, "adopt-only")?;
    adopt.name = Some(LeaseName::new("shared-name")?);
    adopt.idempotency_key = LeaseRequestKey::new("another-request")?;
    let tenant = adopt.tenant.clone();
    let LeaseResponse::Granted(adopted) = core.handle_lease_request(adopt).await else {
        return Err("adopt existing lease".into());
    };
    assert!(!adopted.created.is_created());
    assert_eq!(adopted.lease_id, original.lease_id);
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "adopt-only")?).await,
        CancelOperationResponse::AlreadyCompleted {
            state: OperationState::Succeeded
        }
    );
    assert_eq!(
        core.active_leases
            .get(&original.lease_id)
            .await?
            .ok_or("the value is missing")?
            .status,
        LeaseStatus::Running
    );
    assert_eq!(bus.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_after_success_discloses_the_live_grant_without_deleting_it() -> TestResult {
    let bus = LaunchBus::new(false);
    let core = durable_core(bus.clone(), op_store(), "broker-a")?;
    let request = create_with(ProviderKind::Mock, "completed-before-cancel")?;
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
        return Err("completed create must return its owned grant".into());
    };
    assert!(grant.created.is_created());
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "completed-before-cancel")?)
            .await,
        CancelOperationResponse::AlreadyCompleted {
            state: OperationState::Succeeded
        }
    );
    let GetOperationResponse::Found { operation } = core
        .handle_get_operation(GetOperationRequest {
            tenant,
            operation_id: op_id("completed-before-cancel")?,
        })
        .await
    else {
        return Err("authoritative operation outcome must remain readable".into());
    };
    let Some(OperationResult::Granted(stored)) = operation.result else {
        return Err("exact granted lease remains disclosed".into());
    };
    assert_eq!(stored.lease_id, grant.lease_id);
    assert!(stored.created.is_created());
    assert_eq!(
        core.active_leases
            .get(&grant.lease_id)
            .await?
            .ok_or("the value is missing")?
            .status,
        LeaseStatus::Running
    );
    assert_eq!(bus.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_intent_from_another_tenant_cannot_cancel_the_owner() -> TestResult {
    let bus = LaunchBus::new(false);
    let core = durable_core(bus.clone(), op_store(), "broker-a")?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&TenantId::new("foreign")?, "shared-operation-id")?)
            .await,
        CancelOperationResponse::Pending
    );
    assert!(matches!(
        core.handle_lease_request(create_with(ProviderKind::Mock, "shared-operation-id")?)
            .await,
        LeaseResponse::Granted(_)
    ));
    assert_eq!(bus.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_lost_cleanup_ack_remains_pending_across_restart() -> TestResult {
    let bus = CancelableLaunchBus::new();
    bus.cleanup_fails.store(true, Ordering::SeqCst);
    let store = op_store();
    let mut core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    use_targeted_worker(&mut core, ProviderKind::Fly).await?;
    let request = create_with(ProviderKind::Fly, "cancel-recover")?;
    let tenant = request.tenant.clone();
    let core = Arc::new(core);
    let running = core.clone();
    let create = tokio::spawn(async move { running.handle_lease_request(request).await });
    tokio::time::timeout(Duration::from_secs(3), bus.entered.notified()).await?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "cancel-recover")?).await,
        CancelOperationResponse::Pending
    );
    bus.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), bus.cleanup_entered.notified()).await?;
    create.abort();
    let _ = create.await;
    assert!(
        !store_record(&store, &tenant, "cancel-recover")
            .await?
            .state
            .is_terminal()
    );
    bus.cleanup_fails.store(false, Ordering::SeqCst);
    let recovered = restarted(&core, bus.clone(), store.clone())?;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            recovered.recover_operations().await;
            if store_record(&store, &tenant, "cancel-recover").await?.state == OperationState::Canceled {
                break Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| "exact owned cleanup must resume after restart")??;
    let cleaned = locked(&bus.cleaned)?;
    assert!(cleaned.len() >= 2);
    assert!(cleaned.iter().all(|identity| identity == &cleaned[0]));
    assert_eq!(bus.inner.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_capability_and_requested_operation_id_fail_closed_when_disabled() -> TestResult {
    let bus = LaunchBus::new(false);
    let disabled = make_core_with_bus(bus.clone())?;
    let request = create_with(ProviderKind::Mock, "must-be-durable")?;
    assert!(
        !disabled
            .handle_operation_capabilities(OperationCapabilitiesRequest {
                tenant: request.tenant.clone()
            })
            .await
            .supports_create_cancellation
            .is_supported()
    );
    assert!(matches!(
        disabled.handle_lease_request(request.clone()).await,
        LeaseResponse::Denied(_)
    ));
    assert_eq!(bus.launches()?, 0);
    let enabled = durable_core(bus, op_store(), "broker-a")?;
    assert!(
        enabled
            .handle_operation_capabilities(OperationCapabilitiesRequest { tenant: request.tenant })
            .await
            .supports_create_cancellation
            .is_supported()
    );
    Ok(())
}

#[tokio::test]
async fn operation_cancel_legacy_e2b_requires_exact_owned_resource_acknowledgement() -> TestResult {
    let bus = CancelableLaunchBus::new();
    let store = op_store();
    let core = Arc::new(durable_core(bus.clone(), store.clone(), "broker-a")?);
    let request = create_with(ProviderKind::E2b, "legacy-owned-cancel")?;
    let tenant = request.tenant.clone();
    let running = core.clone();
    let create = tokio::spawn(async move { running.handle_lease_request(request).await });
    tokio::time::timeout(Duration::from_secs(3), bus.entered.notified()).await?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "legacy-owned-cancel")?)
            .await,
        CancelOperationResponse::Pending
    );
    bus.release.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(5), create)
        .await
        .map_err(|_| "a known legacy allocation needs an acknowledged owned cleanup path")??;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::OperationCanceled { .. })
    ));
    assert_eq!(
        store_record(&store, &tenant, "legacy-owned-cancel").await?.state,
        OperationState::Canceled
    );
    let cleaned = locked(&bus.owned_cleaned)?;
    assert_eq!(cleaned.len(), 1);
    assert_eq!(cleaned[0].tenant, tenant);
    assert_eq!(bus.inner.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_legacy_lost_owned_cleanup_ack_survives_broker_reopen() -> TestResult {
    let bus = CancelableLaunchBus::new();
    bus.cleanup_fails.store(true, Ordering::SeqCst);
    let store = op_store();
    let core = Arc::new(durable_core(bus.clone(), store.clone(), "broker-a")?);
    let request = create_with(ProviderKind::E2b, "legacy-owned-retry")?;
    let tenant = request.tenant.clone();
    let running = core.clone();
    let create = tokio::spawn(async move { running.handle_lease_request(request).await });
    tokio::time::timeout(Duration::from_secs(3), bus.entered.notified()).await?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "legacy-owned-retry")?)
            .await,
        CancelOperationResponse::Pending
    );
    bus.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), bus.cleanup_entered.notified())
        .await
        .map_err(|_| "legacy cancellation must reach the owned request-reply seam")?;
    create.abort();
    let _ = create.await;
    assert!(
        !store_record(&store, &tenant, "legacy-owned-retry")
            .await?
            .state
            .is_terminal()
    );
    bus.cleanup_fails.store(false, Ordering::SeqCst);
    let recovered = restarted(&core, bus.clone(), store.clone())?;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            recovered.recover_operations().await;
            if store_record(&store, &tenant, "legacy-owned-retry").await?.state == OperationState::Canceled {
                break Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| "owned legacy cleanup must resume from durable custody")??;
    let cleaned = locked(&bus.owned_cleaned)?;
    assert!(cleaned.len() >= 2);
    assert!(cleaned.iter().all(|identity| identity == &cleaned[0]));
    assert_eq!(bus.inner.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_provider_denial_cannot_invent_host_cancellation_intent() -> TestResult {
    let store = op_store();
    let id = op_id("provider-cannot-cancel")?;
    let bus = Arc::new(MockBus {
        provider_response: freeze_reply(SandboxFreezeResponse::Denied(LeaseError::OperationCanceled {
            operation_id: id.clone(),
        }))?,
    });
    let core = durable_core(bus, store.clone(), "broker-a")?;
    let tenant = TenantId::new("owner")?;
    let lease_id = LeaseId::new("owned-pause-lease");
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert!(matches!(
        core.handle_pause_lease(pause_with(&tenant, &lease_id, id.as_str(), 600)?)
            .await,
        PauseLeaseResponse::Denied(LeaseError::OperationCanceled { .. })
    ));
    let operation = store_record(&store, &tenant, id.as_str()).await?;
    assert_eq!(
        operation.state,
        OperationState::Failed,
        "only a recorded host cancellation may enter Canceled"
    );
    assert!(operation.cancellation.is_none());
    Ok(())
}

#[tokio::test]
async fn operation_cancel_precancel_tombstone_does_not_poison_a_noncreate_operation() -> TestResult {
    let bus = FreezeWorkerBus::new(false);
    let core = fresh_core(bus.clone())?;
    let tenant = TenantId::new("owner")?;
    let lease = running_lease_in(&core, &tenant, "retained-pause-target").await?;
    assert_eq!(
        core.handle_cancel_operation(cancel(&tenant, "future-pause")?).await,
        CancelOperationResponse::Pending
    );
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        core.handle_pause_lease(pause_with(&tenant, &lease, "future-pause", 600)?),
    )
    .await
    .map_err(|_| "unsupported cancellation must not poison the future lifecycle request")?;
    assert!(matches!(response, PauseLeaseResponse::Paused { .. }), "{response:?}");
    let GetOperationResponse::Found { operation } = core
        .handle_get_operation(GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: op_id("future-pause")?,
        })
        .await
    else {
        return Err("pause result recorded".into());
    };
    assert_eq!(operation.state, OperationState::Succeeded);
    assert!(operation.cancellation.is_none());
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

struct FinishBarrierStore {
    inner: Arc<dyn crate::operations::OperationStore>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    armed: AtomicBool,
    stale_running_read: AtomicBool,
}

#[async_trait::async_trait]
impl crate::operations::OperationStore for FinishBarrierStore {
    async fn get(&self, key: &str) -> Result<Option<crate::operations::Entry>, crate::operations::OperationStoreError> {
        let found = self.inner.get(key).await?;
        if key.starts_with("op.")
            && found.as_ref().is_some_and(|entry| {
                serde_json::from_slice::<OperationRecord>(&entry.value)
                    .is_ok_and(|record| record.state == OperationState::Running)
            })
            && self.stale_running_read.swap(false, Ordering::SeqCst)
        {
            self.release.notify_one();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if self.inner.get(key).await?.as_ref().is_some_and(|entry| {
                        serde_json::from_slice::<OperationRecord>(&entry.value)
                            .is_ok_and(|record| record.state == OperationState::Succeeded)
                    }) {
                        break Ok::<(), crate::operations::OperationStoreError>(());
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .map_err(crate::operations::OperationStoreError::backend)??;
        }
        Ok(found)
    }
    async fn create(&self, key: &str, value: Bytes) -> Result<Option<u64>, crate::operations::OperationStoreError> {
        self.inner.create(key, value).await
    }
    async fn update(
        &self,
        key: &str,
        value: Bytes,
        revision: u64,
    ) -> Result<Option<u64>, crate::operations::OperationStoreError> {
        if serde_json::from_slice::<OperationRecord>(&value)
            .is_ok_and(|record| record.state == OperationState::Succeeded)
            && self.armed.swap(false, Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.update(key, value, revision).await
    }
    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<bool, crate::operations::OperationStoreError> {
        self.inner.delete(key, revision).await
    }
    async fn keys(&self, prefix: &str) -> Result<Vec<String>, crate::operations::OperationStoreError> {
        self.inner.keys(prefix).await
    }
}

#[tokio::test]
async fn operation_cancel_same_record_cas_fences_a_racing_successful_finish() -> TestResult {
    let store = Arc::new(FinishBarrierStore {
        inner: op_store(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        armed: AtomicBool::new(true),
        stale_running_read: AtomicBool::new(false),
    });
    let operations = Arc::new(Operations::new(store.clone(), "broker-a"));
    let tenant = TenantId::new("owner")?;
    let id = op_id("finish-race")?;
    let lease = LeaseId::new("finish-race-lease");
    operations
        .begin(
            &tenant,
            &id,
            &lease,
            OperationRequest::Create {
                request: Box::new(create_with(ProviderKind::Mock, id.as_str())?),
                attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("exact-attempt")?,
                dispatch: None,
                quota: None,
            },
            SystemTime::now(),
        )
        .await?;
    let (_, token) = operations
        .claim(&tenant, &id, SystemTime::now())
        .await?
        .ok_or("the value is missing")?;
    let finishing = operations.clone();
    let finish = tokio::spawn(async move {
        finishing
            .finish(&token, Ok(OperationResult::Unsupported), SystemTime::now())
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), store.entered.notified()).await?;
    operations.request_cancel(&tenant, &id, SystemTime::now()).await?;
    store.release.notify_one();
    assert!(matches!(finish.await??, Settled::Fenced));
    let operation = operations
        .get(&tenant, &id)
        .await?
        .ok_or("the value is missing")?
        .record;
    assert_eq!(operation.state, OperationState::Running);
    assert!(operation.cancellation.is_some());
    assert!(operation.result.is_none());
    Ok(())
}

#[tokio::test]
async fn operation_cancel_compensates_only_an_opted_in_completed_owned_create() -> TestResult {
    let bus = CancelableLaunchBus::new();
    bus.release.notify_one();
    let store = op_store();
    let core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let request = create_with(ProviderKind::E2b, "completed-owned-compensation")?;
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
        return Err("owned create completed".into());
    };
    assert!(grant.created.is_created());
    let sibling = running_source_record(&tenant, &LeaseId::new("compensation-sibling"))?;
    core.active_leases.put(&sibling).await?;
    assert_eq!(
        core.handle_cancel_operation(compensate(&tenant, "completed-owned-compensation")?)
            .await,
        CancelOperationResponse::Canceled
    );
    let GetOperationResponse::Found { operation: status } = core
        .handle_get_operation(GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: op_id("completed-owned-compensation")?,
        })
        .await
    else {
        return Err("compensation status remains readable".into());
    };
    assert_eq!(
        status.compensation,
        Some(trogon_sandbox_contract::OperationCompensationState::Cleaned)
    );
    let operation = store_record(&store, &tenant, "completed-owned-compensation").await?;
    assert_eq!(
        operation.state,
        OperationState::Succeeded,
        "compensation must preserve the immutable original result"
    );
    assert!(
        matches!(operation.result, Some(OperationResult::Granted(ref stored)) if stored.lease_id == grant.lease_id && stored.created.is_created())
    );
    assert!(
        core.active_leases
            .get(&grant.lease_id)
            .await?
            .is_none_or(|record| record.status == LeaseStatus::Released)
    );
    assert_eq!(
        core.active_leases
            .get(&sibling.lease_id)
            .await?
            .ok_or("the value is missing")?,
        sibling
    );
    assert_eq!(locked(&bus.owned_cleaned)?.len(), 1);
    assert_eq!(
        core.handle_cancel_operation(compensate(&tenant, "completed-owned-compensation")?)
            .await,
        CancelOperationResponse::Canceled
    );
    assert_eq!(locked(&bus.owned_cleaned)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_compensation_preserves_an_adopted_named_lease() -> TestResult {
    let bus = LaunchBus::new(false);
    let core = durable_core(bus.clone(), op_store(), "broker-a")?;
    let mut initial = create_with(ProviderKind::Mock, "named-original")?;
    initial.name = Some(LeaseName::new("retained-adopted")?);
    let LeaseResponse::Granted(original) = core.handle_lease_request(initial).await else {
        return Err("initial named lease".into());
    };
    let mut adoption = create_with(ProviderKind::Mock, "named-adoption")?;
    adoption.name = Some(LeaseName::new("retained-adopted")?);
    adoption.idempotency_key = LeaseRequestKey::new("new-adoption-key")?;
    let tenant = adoption.tenant.clone();
    let LeaseResponse::Granted(adopted) = core.handle_lease_request(adoption).await else {
        return Err("named adoption".into());
    };
    assert!(!adopted.created.is_created());
    assert_eq!(
        core.handle_cancel_operation(compensate(&tenant, "named-adoption")?)
            .await,
        CancelOperationResponse::AlreadyCompleted {
            state: OperationState::Succeeded
        }
    );
    assert_eq!(adopted.lease_id, original.lease_id);
    assert_eq!(
        core.active_leases
            .get(&original.lease_id)
            .await?
            .ok_or("the value is missing")?
            .status,
        LeaseStatus::Running
    );
    assert_eq!(bus.launches()?, 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_compensation_cannot_retire_a_changed_resource_or_epoch() -> TestResult {
    for changed_epoch in [false, true] {
        let bus = CancelableLaunchBus::new();
        bus.release.notify_one();
        let mut core = durable_core(bus.clone(), op_store(), "broker-a")?;
        let provider = if changed_epoch {
            use_targeted_worker(&mut core, ProviderKind::Fly).await?;
            ProviderKind::Fly
        } else {
            ProviderKind::E2b
        };
        let id = if changed_epoch {
            "changed-epoch"
        } else {
            "changed-resource"
        };
        let request = create_with(provider, id)?;
        let tenant = request.tenant.clone();
        let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
            return Err("owned create completed".into());
        };
        let mut replacement = core
            .active_leases
            .get(&grant.lease_id)
            .await?
            .ok_or("the value is missing")?;
        if changed_epoch {
            replacement
                .dispatch_attempt
                .as_mut()
                .ok_or("the dispatch attempt is missing")?
                .epoch = Some(trogon_sandbox_contract::LeaseEpoch::new(999)?);
        } else {
            replacement.sandbox_id = "another-native-allocation".to_string();
        }
        core.active_leases.put(&replacement).await?;
        assert_eq!(
            core.handle_cancel_operation(compensate(&tenant, id)?).await,
            CancelOperationResponse::Pending
        );
        assert_eq!(
            core.active_leases
                .get(&grant.lease_id)
                .await?
                .ok_or("the value is missing")?,
            replacement
        );
        assert!(locked(&bus.cleaned)?.is_empty());
        assert!(locked(&bus.owned_cleaned)?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn operation_cancel_compensation_lost_ack_retains_exact_custody_after_reopen() -> TestResult {
    let bus = CancelableLaunchBus::new();
    bus.release.notify_one();
    let store = op_store();
    let core = durable_core(bus.clone(), store.clone(), "broker-a")?;
    let request = create_with(ProviderKind::E2b, "compensation-retry")?;
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
        return Err("owned create completed".into());
    };
    bus.cleanup_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        core.handle_cancel_operation(compensate(&tenant, "compensation-retry")?)
            .await,
        CancelOperationResponse::Pending
    );
    let GetOperationResponse::Found { operation: status } = core
        .handle_get_operation(GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: op_id("compensation-retry")?,
        })
        .await
    else {
        return Err("pending compensation status remains readable".into());
    };
    assert_eq!(
        status.compensation,
        Some(trogon_sandbox_contract::OperationCompensationState::Pending)
    );
    assert_eq!(
        store_record(&store, &tenant, "compensation-retry").await?.state,
        OperationState::Succeeded
    );
    bus.cleanup_fails.store(false, Ordering::SeqCst);
    let recovered = restarted(&core, bus.clone(), store)?;
    assert_eq!(
        recovered
            .handle_cancel_operation(compensate(&tenant, "compensation-retry")?)
            .await,
        CancelOperationResponse::Canceled
    );
    let cleaned = locked(&bus.owned_cleaned)?;
    assert_eq!(cleaned.len(), 2);
    assert!(cleaned.iter().all(|request| request == &cleaned[0]));
    assert_eq!(cleaned[0].lease_id, grant.lease_id);
    assert_eq!(cleaned[0].sandbox_id.as_str(), grant.sandbox_id);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_targeted_completed_named_create_retires_only_its_owned_name() -> TestResult {
    let bus = CancelableLaunchBus::new();
    bus.release.notify_one();
    let mut core = durable_core(bus.clone(), op_store(), "broker-a")?;
    use_targeted_worker(&mut core, ProviderKind::Fly).await?;
    let mut request = create_with(ProviderKind::Fly, "named-target-compensation")?;
    request.name = Some(LeaseName::new("fresh-target-name")?);
    let tenant = request.tenant.clone();
    let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
        return Err("fresh named target created".into());
    };
    assert!(grant.created.is_created());
    assert_eq!(
        core.named_leases.get(&tenant, "fresh-target-name").await?,
        Some(grant.lease_id.clone())
    );
    let sibling = LeaseId::new("another-name-holder");
    core.named_leases
        .claim(&tenant, "retained-other-name", &sibling, Duration::from_secs(300))
        .await?;
    assert_eq!(
        core.handle_cancel_operation(compensate(&tenant, "named-target-compensation")?)
            .await,
        CancelOperationResponse::Canceled
    );
    assert_eq!(
        core.named_leases.get(&tenant, "fresh-target-name").await?,
        None,
        "positive cleanup must retire the exact created name before reporting completion"
    );
    assert_eq!(
        core.named_leases.get(&tenant, "retained-other-name").await?,
        Some(sibling)
    );
    assert_eq!(locked(&bus.cleaned)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn operation_cancel_compensates_when_success_wins_between_lookup_and_cancel_intent() -> TestResult {
    let bus = CancelableLaunchBus::new();
    bus.release.notify_one();
    let store = Arc::new(FinishBarrierStore {
        inner: op_store(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        armed: AtomicBool::new(true),
        stale_running_read: AtomicBool::new(false),
    });
    let core = Arc::new(durable_core(bus.clone(), store.clone(), "broker-a")?);
    let request = create_with(ProviderKind::E2b, "late-complete-before-cancel-cas")?;
    let tenant = request.tenant.clone();
    let running = core.clone();
    let create = tokio::spawn(async move { running.handle_lease_request(request).await });
    tokio::time::timeout(Duration::from_secs(3), store.entered.notified())
        .await
        .map_err(|_| "successful finish reached its CAS barrier")?;
    store.stale_running_read.store(true, Ordering::SeqCst);
    assert_eq!(
        core.handle_cancel_operation(compensate(&tenant, "late-complete-before-cancel-cas")?)
            .await,
        CancelOperationResponse::Canceled,
        "an opted-in cancel must compensate a fresh success observed after its initial Running lookup"
    );
    let LeaseResponse::Granted(grant) = create.await? else {
        return Err("the original operation already succeeded".into());
    };
    assert!(grant.created.is_created());
    assert_eq!(locked(&bus.owned_cleaned)?.len(), 1);
    assert_eq!(locked(&bus.owned_cleaned)?[0].lease_id, grant.lease_id);
    let GetOperationResponse::Found { operation } = core
        .handle_get_operation(GetOperationRequest {
            tenant,
            operation_id: op_id("late-complete-before-cancel-cas")?,
        })
        .await
    else {
        return Err("original result remains readable".into());
    };
    assert_eq!(operation.state, OperationState::Succeeded);
    assert_eq!(
        operation.compensation,
        Some(trogon_sandbox_contract::OperationCompensationState::Cleaned)
    );
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod nats_recovery;
