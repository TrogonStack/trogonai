use super::*;

pub(super) fn captured_dispatch(
    bus: &CapturingBus,
) -> anyhow::Result<trogon_sandbox_contract::ProviderDispatchPayload> {
    let payload = locked(&bus.last_payload)?.clone().context("dispatch payload")?;
    Ok(serde_json::from_slice(&payload)?)
}

pub(super) fn native_handle_reply() -> anyhow::Result<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(
        &trogon_sandbox_contract::NativeHandleResponse::Available(trogon_sandbox_contract::NativeHandle {
            provider: ProviderKind::Mock,
            vendor_id: "vendor-1".to_string(),
            url: None,
            region: None,
        }),
    )?))
}

pub(super) fn ssh_access_reply() -> anyhow::Result<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(&SshAccessResponse::Available(
        trogon_sandbox_contract::SshAccess {
            host: "ssh.example".to_string(),
            port: 22,
            user: "token-1".to_string(),
            command: None,
            expires_at: None,
        },
    ))?))
}

/// A freeze worker that is idempotent like the real ones. The first call
/// can be made to hang after acting, which is what a broker crash between
/// the worker acting and the reply looks like from the broker's side.
pub(super) struct FreezeWorkerBus {
    pub(super) calls: AtomicU32,
    pub(super) effective: AtomicU32,
    pub(super) frozen: std::sync::atomic::AtomicBool,
    pub(super) hang_after_acting: std::sync::atomic::AtomicBool,
}

impl FreezeWorkerBus {
    pub(super) fn new(hang_after_acting: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicU32::new(0),
            effective: AtomicU32::new(0),
            frozen: std::sync::atomic::AtomicBool::new(false),
            hang_after_acting: std::sync::atomic::AtomicBool::new(hang_after_acting),
        })
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for FreezeWorkerBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.frozen.swap(true, Ordering::SeqCst) {
            self.effective.fetch_add(1, Ordering::SeqCst);
        }
        if self.hang_after_acting.swap(false, Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        freeze_reply(SandboxFreezeResponse::Done)
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn core_over(
    bus: Arc<dyn crate::bus::BrokerBus>,
    active: Arc<dyn ActiveLeaseStore>,
    store: Arc<dyn crate::operations::OperationStore>,
    owner: &str,
) -> anyhow::Result<BrokerCore> {
    let mut core = CoreParts::new(bus).build()?;
    core.active_leases = active;
    Ok(core.with_operations(Arc::new(crate::operations::Operations::new(store, owner))))
}

pub(super) fn fresh_core(bus: Arc<dyn crate::bus::BrokerBus>) -> anyhow::Result<BrokerCore> {
    core_over(
        bus,
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(crate::operations::InMemoryOperationStore::new()),
        "broker-a",
    )
}

pub(super) fn op_id(raw: &str) -> anyhow::Result<OperationId> {
    Ok(OperationId::new(raw)?)
}

pub(super) fn pause_with(tenant: &TenantId, lease: &LeaseId, id: &str, secs: u32) -> anyhow::Result<PauseLeaseRequest> {
    Ok(PauseLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease.clone(),
        max_paused_seconds: Some(secs),
        operation_id: Some(op_id(id)?),
    })
}

pub(super) async fn store_record(
    store: &Arc<dyn crate::operations::OperationStore>,
    tenant: &TenantId,
    id: &str,
) -> anyhow::Result<OperationRecord> {
    let found = crate::operations::Operations::new(store.clone(), "reader")
        .get(tenant, &op_id(id)?)
        .await?
        .context("operation recorded")?;
    Ok(found.record)
}

// ---- durable create and restore ----

/// A worker that launches once per dedupe key: the attempt id for a targeted
/// dispatch, the lease id for a legacy one (what a `RedispatchGuard` does).
pub(super) struct LaunchBus {
    pub(super) launched: std::sync::Mutex<std::collections::HashSet<String>>,
    pub(super) receipts: std::sync::Mutex<HashMap<String, CoreProviderLease>>,
    pub(super) lease_ids: std::sync::Mutex<Vec<String>>,
    pub(super) calls: AtomicU32,
    pub(super) hang_after_acting: std::sync::atomic::AtomicBool,
}

impl LaunchBus {
    pub(super) fn new(hang_after_acting: bool) -> Arc<Self> {
        Arc::new(Self {
            launched: Default::default(),
            receipts: Default::default(),
            lease_ids: Default::default(),
            calls: AtomicU32::new(0),
            hang_after_acting: std::sync::atomic::AtomicBool::new(hang_after_acting),
        })
    }

    pub(super) fn launches(&self) -> anyhow::Result<usize> {
        Ok(locked(&self.launched)?.len())
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for LaunchBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if let Ok(cleanup) = serde_json::from_slice::<trogon_sandbox_contract::ProviderCleanupRequest>(&payload) {
            let response = trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
                identity: cleanup.identity,
                intent: cleanup.intent,
            };
            return Ok(serde_json::to_vec(&response)?.into());
        }
        let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        locked(&self.lease_ids)?.push(dispatch.lease_id.as_str().to_string());
        let key = match &dispatch.target {
            Some(target) => target.attempt_id.to_string(),
            None => dispatch.lease_id.as_str().to_string(),
        };
        locked(&self.launched)?.insert(key.clone());
        let lease = CoreProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(format!("sandbox-{key}"))?,
            evidence_subject: "sandbox.evidence.x.>".to_string(),
            worker_id: dispatch.target.as_ref().map(|t| t.worker_id.clone()),
            attempt_id: dispatch.target.as_ref().map(|t| t.attempt_id.clone()),
        };
        locked(&self.receipts)?.insert(dispatch.lease_id.as_str().to_string(), lease.clone());
        if self.hang_after_acting.swap(false, Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(Bytes::from(serde_json::to_vec(&lease)?))
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn durable_core(
    bus: Arc<dyn crate::bus::BrokerBus>,
    store: Arc<dyn crate::operations::OperationStore>,
    owner: &str,
) -> anyhow::Result<BrokerCore> {
    Ok(make_core_with_bus(bus)?.with_operations(Arc::new(crate::operations::Operations::new(store, owner))))
}

/// A second broker process over the same durable stores.
pub(super) fn restarted(
    first: &BrokerCore,
    bus: Arc<dyn crate::bus::BrokerBus>,
    store: Arc<dyn crate::operations::OperationStore>,
) -> anyhow::Result<BrokerCore> {
    let mut core = durable_core(bus, store, "broker-b")?;
    core.quota = first.quota.clone();
    core.idempotency = first.idempotency.clone();
    core.active_leases = first.active_leases.clone();
    core.drive_store = first.drive_store.clone();
    core.named_leases = first.named_leases.clone();
    core.snapshot_pins = first.snapshot_pins.clone();
    core.resolved_digests = first.resolved_digests.clone();
    core.worker_registry = first.worker_registry.clone();
    Ok(core)
}

pub(super) async fn use_targeted_worker(core: &mut BrokerCore, provider: ProviderKind) -> anyhow::Result<()> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut heartbeat = worker_heartbeat("durable-create-worker", provider, default_capability_profile())?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    Ok(())
}

pub(super) fn create_with(provider: ProviderKind, op: &str) -> anyhow::Result<LeaseRequest> {
    let mut request = make_request(vec![provider])?;
    request.operation_id = Some(op_id(op)?);
    Ok(request)
}

pub(super) fn op_store() -> Arc<dyn crate::operations::OperationStore> {
    Arc::new(crate::operations::InMemoryOperationStore::new())
}

pub(super) async fn crash_mid_create(first: &BrokerCore, request: LeaseRequest) {
    let attempt = tokio::time::timeout(Duration::from_millis(300), first.handle_lease_request(request)).await;
    assert!(attempt.is_err(), "the first broker never answered");
}

pub(super) async fn quota_usage(core: &BrokerCore, tenant: &TenantId) -> anyhow::Result<(u32, u32)> {
    let record = core.quota.quota_record(tenant).await?.context("quota record")?;
    Ok((record.cpu_seconds_used, record.active_lease_count()))
}
