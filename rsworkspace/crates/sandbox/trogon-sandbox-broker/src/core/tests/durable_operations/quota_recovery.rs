use super::*;
use crate::operations::NatsKvOperationStore;
use crate::quota::NatsKvQuotaStore;
use trogon_sandbox_contract::{CreateQuotaStep, LeaseAccounting};

#[derive(Clone, Copy)]
enum QuotaStepCut {
    Reserved,
    Counted,
    Cleared,
}

struct InterruptedQuotaStepStore {
    inner: Arc<dyn OperationStore>,
    operation: OperationId,
    step: QuotaStepCut,
    remaining: AtomicU32,
    entered: tokio::sync::Notify,
}

impl InterruptedQuotaStepStore {
    fn new(operation: &str, step: QuotaStepCut, remaining: u32) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: Arc::new(InMemoryOperationStore::new()),
            operation: op_id(operation)?,
            step,
            remaining: AtomicU32::new(remaining),
            entered: tokio::sync::Notify::new(),
        }))
    }

    fn interrupts(&self, previous: &OperationRecord, next: &OperationRecord) -> bool {
        if next.id != self.operation {
            return false;
        }
        let Some(OperationRequest::Create { quota: before, .. }) = &previous.request else {
            return false;
        };
        let after = match &next.request {
            Some(OperationRequest::Create { quota, .. }) if next.state == OperationState::Running => quota,
            None if next.state.is_terminal() && matches!(self.step, QuotaStepCut::Cleared) => &None,
            _ => return false,
        };
        match self.step {
            QuotaStepCut::Reserved => before.is_none() && matches!(after, Some(CreateQuotaStep::Reserved { .. })),
            QuotaStepCut::Counted => {
                !matches!(before, Some(CreateQuotaStep::Counted)) && matches!(after, Some(CreateQuotaStep::Counted))
            }
            QuotaStepCut::Cleared => {
                matches!(
                    before,
                    Some(CreateQuotaStep::Reserved { .. } | CreateQuotaStep::Releasing { .. })
                ) && after.is_none()
            }
        }
    }
}

#[async_trait::async_trait]
impl OperationStore for InterruptedQuotaStepStore {
    async fn get(&self, key: &str) -> Result<Option<Entry>, crate::operations::OperationStoreError> {
        self.inner.get(key).await
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
        if let Some(previous) = self.inner.get(key).await?
            && let (Ok(before), Ok(after)) = (
                serde_json::from_slice::<OperationRecord>(&previous.value),
                serde_json::from_slice::<OperationRecord>(&value),
            )
            && self.interrupts(&before, &after)
            && self
                .remaining
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
                .is_ok()
        {
            self.entered.notify_one();
            std::future::pending::<()>().await;
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

struct InterruptedWorkerReply {
    inner: Arc<LaunchBus>,
    interrupt: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
}

impl InterruptedWorkerReply {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: LaunchBus::new(false),
            interrupt: std::sync::atomic::AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for InterruptedWorkerReply {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        let reply = self.inner.request(subject, payload).await?;
        if self.interrupt.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(reply)
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.inner.publish(subject, payload).await
    }
}

pub(super) async fn interrupt_quota_drive<T: std::fmt::Debug + Send + 'static>(
    barrier: &tokio::sync::Notify,
    drive: impl std::future::Future<Output = T> + Send + 'static,
) -> anyhow::Result<Option<T>> {
    let mut task = tokio::spawn(drive);
    tokio::select! {
        _ = barrier.notified() => {
            task.abort();
            let outcome = task.await;
            assert!(matches!(outcome, Err(ref error) if error.is_cancelled()));
            Ok(None)
        }
        result = &mut task => Ok(Some(result?)),
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            task.abort();
            let _ = task.await;
            Err(anyhow::anyhow!("public broker drive neither reached the crash barrier nor completed"))
        }
    }
}

fn quota_restarted(
    previous: &BrokerCore,
    bus: Arc<dyn crate::bus::BrokerBus>,
    store: Arc<dyn OperationStore>,
    owner: &str,
) -> anyhow::Result<Arc<BrokerCore>> {
    Ok(Arc::new(restarted(previous, bus, store.clone())?.with_operations(
        Arc::new(crate::operations::Operations::new(store, owner)),
    )))
}

pub(super) async fn durable_quota_values(core: &BrokerCore, tenant: &TenantId) -> anyhow::Result<(u32, u32, u32)> {
    Ok(core
        .quota
        .quota_record(tenant)
        .await?
        .map(|record| {
            (
                record.cpu_seconds_used,
                record.wall_seconds_used,
                record.active_lease_count(),
            )
        })
        .unwrap_or_default())
}

pub(super) async fn reconcile_positive_legacy_receipt(
    core: &BrokerCore,
    bus: &LaunchBus,
    store: &Arc<dyn OperationStore>,
    tenant: &TenantId,
    operation: &str,
) -> TestResult {
    let key = crate::operations::op_key(tenant, &op_id(operation)?);
    let found = store.get(&key).await?.ok_or("record missing")?;
    let mut operation: OperationRecord = serde_json::from_slice(&found.value)?;
    let lease_id = operation.target.clone().ok_or("operation has no target lease")?;
    let Some(OperationRequest::Create {
        request,
        attempt_id,
        quota: Some(CreateQuotaStep::Reserved {
            reservation: Some(reservation),
            ..
        }),
        ..
    }) = &operation.request
    else {
        return Err("positive reconciliation requires the original typed reservation".into());
    };
    assert_eq!(&reservation.attempt_id, attempt_id);
    assert_eq!(reservation.created_at, operation.requested_at);
    let receipt = bus
        .receipts
        .lock()
        .map_err(|_| "poisoned lock")?
        .get(lease_id.as_str())
        .cloned()
        .ok_or("the original worker observed this exact allocation receipt")?;
    assert!(
        bus.launched
            .lock()
            .map_err(|_| "poisoned lock")?
            .contains(lease_id.as_str())
    );
    let mut active = running_source_record(tenant, &lease_id)?;
    active.provider = request.provider_pref[0];
    active.sandbox_id = receipt.sandbox_id.to_string();
    active.worker_id = receipt.worker_id;
    active.spec = request.spec.clone();
    active.name = request.name.clone();
    active.tags = request.tags.clone();
    active.created_at = operation.requested_at;
    active.deadline = operation.requested_at + Duration::from_secs(u64::from(request.ttl_seconds));
    active.cpu_seconds_consumed = reservation.cpu_seconds;
    active.wall_seconds_consumed = reservation.wall_seconds;
    active.quota_reservation = Some(reservation.clone());
    core.active_leases.put(&active).await?;
    operation.next_attempt_at = None;
    assert!(
        store
            .update(&key, serde_json::to_vec(&operation)?.into(), found.revision)
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn durable_quota_count_survives_two_lost_counted_writes() -> TestResult {
    let operation = "quota-count-crash";
    let fault = InterruptedQuotaStepStore::new(operation, QuotaStepCut::Counted, 2)?;
    let store: Arc<dyn OperationStore> = fault.clone();
    let bus = LaunchBus::new(false);
    let tenant = TenantId::new("test-tenant")?;
    let mut core = Arc::new(durable_core(bus.clone(), store.clone(), "quota-a")?);
    let request_core = core.clone();
    let request = create_with(ProviderKind::Mock, operation)?;
    interrupt_quota_drive(&fault.entered, async move {
        request_core.handle_lease_request(request).await
    })
    .await?;
    let mut observed = vec![durable_quota_values(&core, &tenant).await?];
    for owner in ["quota-b", "quota-c", "quota-d"] {
        core = quota_restarted(&core, bus.clone(), store.clone(), owner)?;
        let recovering = core.clone();
        interrupt_quota_drive(&fault.entered, async move { recovering.recover_operations().await }).await?;
        observed.push(durable_quota_values(&core, &tenant).await?);
    }
    assert_eq!(
        store_record(&store, &tenant, operation).await?.state,
        OperationState::Succeeded
    );
    assert_eq!(bus.launches()?, 1);
    assert_eq!(
        observed,
        vec![(60, 300, 1); 4],
        "one operation holds one budget and active slot through repeated recovery"
    );
    Ok(())
}

#[tokio::test]
async fn durable_quota_reservation_survives_a_lost_initial_step_write() -> TestResult {
    let operation = "quota-reserve-crash";
    let fault = InterruptedQuotaStepStore::new(operation, QuotaStepCut::Reserved, 1)?;
    let store: Arc<dyn OperationStore> = fault.clone();
    let bus = LaunchBus::new(false);
    let tenant = TenantId::new("test-tenant")?;
    let first = Arc::new(durable_core(bus.clone(), store.clone(), "quota-a")?);
    let creating = first.clone();
    let request = create_with(ProviderKind::Mock, operation)?;
    interrupt_quota_drive(
        &fault.entered,
        async move { creating.handle_lease_request(request).await },
    )
    .await?;
    let second = quota_restarted(&first, bus.clone(), store.clone(), "quota-b")?;
    second.recover_operations().await;
    let third = quota_restarted(&second, bus.clone(), store.clone(), "quota-c")?;
    third.recover_operations().await;
    assert_eq!(
        store_record(&store, &tenant, operation).await?.state,
        OperationState::Succeeded
    );
    assert_eq!(bus.launches()?, 1);
    assert_eq!(
        durable_quota_values(&third, &tenant).await?,
        (60, 300, 1),
        "a lost operation step must not reserve the same budget twice"
    );
    Ok(())
}

#[tokio::test]
async fn durable_quota_refund_replay_preserves_an_unrelated_reservation() -> TestResult {
    let operation = "quota-refund-crash";
    let fault = InterruptedQuotaStepStore::new(operation, QuotaStepCut::Cleared, 2)?;
    let store: Arc<dyn OperationStore> = fault.clone();
    let bus = InterruptedWorkerReply::new();
    let tenant = TenantId::new("test-tenant")?;
    let mut core = Arc::new(durable_core(bus.clone(), store.clone(), "quota-a")?);
    let mut anchor = make_request(vec![ProviderKind::Mock])?;
    anchor.idempotency_key = LeaseRequestKey::new("quota-anchor-key")?;
    assert!(matches!(
        core.handle_lease_request(anchor).await,
        LeaseResponse::Granted(_)
    ));
    bus.interrupt.store(true, Ordering::SeqCst);
    let creating = core.clone();
    let request = create_with(ProviderKind::Mock, operation)?;
    interrupt_quota_drive(
        &bus.entered,
        async move { creating.handle_lease_request(request).await },
    )
    .await?;
    let mut observed = vec![durable_quota_values(&core, &tenant).await?];
    reconcile_positive_legacy_receipt(&core, &bus.inner, &store, &tenant, operation).await?;
    for owner in ["quota-b", "quota-c", "quota-d"] {
        core = quota_restarted(&core, bus.clone(), store.clone(), owner)?;
        let recovering = core.clone();
        interrupt_quota_drive(&fault.entered, async move { recovering.recover_operations().await }).await?;
        observed.push(durable_quota_values(&core, &tenant).await?);
    }
    assert_eq!(
        store_record(&store, &tenant, operation).await?.state,
        OperationState::Succeeded
    );
    assert_eq!(bus.inner.launches()?, 2);
    assert_eq!(
        durable_quota_values(&core, &tenant).await?,
        (120, 600, 2),
        "replayed refund must preserve the unrelated lease budget; snapshots: {observed:?}"
    );
    Ok(())
}

#[tokio::test]
async fn durable_quota_refund_recovery_preserves_the_original_provider_denial() -> TestResult {
    let operation = "quota-denial-refund";
    let fault = InterruptedQuotaStepStore::new(operation, QuotaStepCut::Cleared, 2)?;
    let store: Arc<dyn OperationStore> = fault.clone();
    let bus = Arc::new(RefusedRedriveBus {
        dispatches: AtomicU32::new(1),
    });
    let tenant = TenantId::new("test-tenant")?;
    let mut core = Arc::new(durable_core(bus.clone(), store.clone(), "denial-a")?);
    let creating = core.clone();
    let request = create_with(ProviderKind::Mock, operation)?;
    interrupt_quota_drive(
        &fault.entered,
        async move { creating.handle_lease_request(request).await },
    )
    .await?;
    assert_eq!(durable_quota_values(&core, &tenant).await?, (0, 0, 0));
    for owner in ["denial-b", "denial-c", "denial-d"] {
        core = quota_restarted(&core, bus.clone(), store.clone(), owner)?;
        let recovering = core.clone();
        interrupt_quota_drive(&fault.entered, async move { recovering.recover_operations().await }).await?;
        assert_eq!(durable_quota_values(&core, &tenant).await?, (0, 0, 0));
    }
    let done = store_record(&store, &tenant, operation).await?;
    assert!(
        matches!(&done.error,
        Some(LeaseError::ProviderLaunchFailed { provider: ProviderKind::Mock, reason }) if reason == "image unavailable"),
        "refund recovery must retain the definitive provider denial: {:?}",
        done.error
    );
    assert_eq!(done.state, OperationState::Failed);
    assert_eq!(
        bus.dispatches.load(Ordering::SeqCst),
        2,
        "a definitive refusal is not dispatched again"
    );
    assert!(core.active_leases.all().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn durable_quota_preflight_denial_keeps_unsettled_ownership_past_the_retry_limit() -> TestResult {
    let operation = "quota-preflight-crash";
    let store = op_store();
    let bus = InterruptedWorkerReply::new();
    let tenant = TenantId::new("test-tenant")?;
    let mut core = Arc::new(durable_core(bus.clone(), store.clone(), "preflight-a")?);
    bus.interrupt.store(true, Ordering::SeqCst);
    let creating = core.clone();
    let request = create_with(ProviderKind::Mock, operation)?;
    interrupt_quota_drive(
        &bus.entered,
        async move { creating.handle_lease_request(request).await },
    )
    .await?;
    assert_eq!(bus.inner.launches()?, 1);
    assert_eq!(durable_quota_values(&core, &tenant).await?, (60, 300, 1));
    for index in 0..crate::operations::MAX_ATTEMPTS + 2 {
        let key = crate::operations::op_key(&tenant, &op_id(operation)?);
        let found = store.get(&key).await?.ok_or("record missing")?;
        let mut due: OperationRecord = serde_json::from_slice(&found.value)?;
        due.next_attempt_at = None;
        assert!(
            store
                .update(&key, Bytes::from(serde_json::to_vec(&due)?), found.revision)
                .await?
                .is_some()
        );
        core = quota_restarted(&core, bus.clone(), store.clone(), &format!("preflight-{index}"))?;
        Arc::get_mut(&mut core)
            .ok_or("the core is still shared")?
            .config
            .default_max_cpu_per_request = 1;
        core.recover_operations().await;
        let waiting = store_record(&store, &tenant, operation).await?;
        assert!(
            !waiting.state.is_terminal(),
            "unsettled worker and quota authority must survive attempt {index}"
        );
        assert!(
            waiting.request.is_some(),
            "recovery must retain the exact create intent"
        );
        assert_eq!(durable_quota_values(&core, &tenant).await?, (60, 300, 1));
        assert_eq!(
            bus.inner.launches()?,
            1,
            "the stricter admission policy must prevent a new dispatch"
        );
    }
    let key = crate::operations::op_key(&tenant, &op_id(operation)?);
    let found = store.get(&key).await?.ok_or("record missing")?;
    let mut due: OperationRecord = serde_json::from_slice(&found.value)?;
    due.next_attempt_at = None;
    assert!(
        store
            .update(&key, Bytes::from(serde_json::to_vec(&due)?), found.revision)
            .await?
            .is_some()
    );
    core = quota_restarted(&core, bus.clone(), store.clone(), "preflight-restored")?;
    reconcile_positive_legacy_receipt(&core, &bus.inner, &store, &tenant, operation).await?;
    core.recover_operations().await;
    let done = store_record(&store, &tenant, operation).await?;
    assert_eq!(done.state, OperationState::Succeeded);
    assert_eq!(
        bus.inner.launches()?,
        1,
        "positive reconciliation preserves the original allocation"
    );
    assert_eq!(bus.inner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(durable_quota_values(&core, &tenant).await?, (60, 300, 1));
    Ok(())
}

#[tokio::test]
async fn durable_quota_nats_replay_and_release_preserve_the_exact_receipt() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let js = async_nats::jetstream::new(server.client.clone());
    for prewarm in [false, true] {
        let operation = if prewarm { "nats-prewarm" } else { "nats-tenant" };
        let bucket = format!("quota-{operation}");
        let durable = Arc::new(NatsKvOperationStore::new(js.clone(), operation, 1).await?);
        let fault = Arc::new(InterruptedQuotaStepStore {
            inner: durable,
            operation: op_id(operation)?,
            step: QuotaStepCut::Counted,
            remaining: AtomicU32::new(2),
            entered: tokio::sync::Notify::new(),
        });
        let store: Arc<dyn OperationStore> = fault.clone();
        let bus = LaunchBus::new(false);
        let tenant = TenantId::new("test-tenant")?;
        let mut first = durable_core(bus.clone(), store.clone(), "nats-a")?;
        first.quota = Arc::new(NatsKvQuotaStore::new(js.clone(), &bucket).await?);
        let mut core = Arc::new(first);
        let mut request = create_with(ProviderKind::Mock, operation)?;
        request.prewarm = LeasePrewarm::from(prewarm);
        let creating = core.clone();
        interrupt_quota_drive(
            &fault.entered,
            async move { creating.handle_lease_request(request).await },
        )
        .await?;
        for owner in ["nats-b", "nats-c", "nats-d"] {
            core = quota_restarted(&core, bus.clone(), store.clone(), owner)?;
            Arc::get_mut(&mut core).ok_or("the core is still shared")?.quota =
                Arc::new(NatsKvQuotaStore::new(js.clone(), &bucket).await?);
            let recovering = core.clone();
            interrupt_quota_drive(&fault.entered, async move { recovering.recover_operations().await }).await?;
            let quota = core.quota.quota_record(&tenant).await?.ok_or("record missing")?;
            assert_eq!(
                (
                    quota.cpu_seconds_used,
                    quota.wall_seconds_used,
                    quota.active_lease_count()
                ),
                if prewarm { (0, 0, 0) } else { (60, 300, 1) }
            );
            assert_eq!(
                (quota.prewarm.active_count, quota.prewarm.total_cpu_seconds),
                if prewarm { (1, 60) } else { (0, 0) }
            );
        }
        let done = store_record(&store, &tenant, operation).await?;
        assert_eq!(done.state, OperationState::Succeeded);
        let Some(OperationResult::Granted(grant)) = done.result else {
            return Err("create is granted".into());
        };
        let mut lease = core.active_leases.get(&grant.lease_id).await?.ok_or("record missing")?;
        let receipt = lease
            .quota_reservation
            .clone()
            .ok_or("durable release retains quota authority")?;
        assert_eq!(receipt.created_at, done.requested_at);
        assert_eq!(
            receipt.accounting,
            if prewarm {
                LeaseAccounting::Prewarm
            } else {
                LeaseAccounting::Tenant
            }
        );
        assert!(
            lease.dispatch_attempt.is_none(),
            "legacy creates have no fabricated dispatch target"
        );
        lease.created_at += Duration::from_secs(86400);
        core.active_leases.put(&lease).await?;
        core.handle_cancel(grant.lease_id, LeaseReleasedReason::Cancelled)
            .await?;
        assert!(!core.quota.release_attempt(&receipt, &core.quota_limits()).await?);
        let quota = core.quota.quota_record(&tenant).await?.ok_or("record missing")?;
        assert_eq!(
            (
                quota.cpu_seconds_used,
                quota.wall_seconds_used,
                quota.active_lease_count()
            ),
            (0, 0, 0)
        );
        assert_eq!((quota.prewarm.active_count, quota.prewarm.total_cpu_seconds), (0, 0));
        assert_eq!(bus.launches()?, 1);
    }
    Ok(())
}
