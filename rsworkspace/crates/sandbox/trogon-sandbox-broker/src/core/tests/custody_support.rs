use super::*;

pub(super) async fn firecracker_format_fixture() -> anyhow::Result<BrokerCore> {
    let mut core = make_core(mock_provider_response()?)?;
    core.bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::Firecracker;
    profile.snapshot.supported_artifact_types = vec![ArtifactType::FirecrackerSnapshotV1];
    let mut heartbeat = worker_heartbeat("fc-format", ProviderKind::Firecracker, profile)?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    Ok(core)
}

pub(super) async fn inject_profile(
    registry: &InMemoryWorkerRegistry,
    worker_id: &str,
    provider: ProviderKind,
    artifacts: Vec<ArtifactType>,
) -> anyhow::Result<()> {
    let mut profile = default_capability_profile();
    profile.provider = provider;
    profile.snapshot.supported_artifact_types = artifacts;
    let mut heartbeat = worker_heartbeat(worker_id, provider, profile)?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::Legacy;
    registry.inject(heartbeat).await;
    Ok(())
}

pub(super) fn snapshots_off() -> trogon_sandbox_contract::RetentionToggles {
    trogon_sandbox_contract::RetentionToggles {
        snapshots: SnapshotCaptureAccess::Refused,
        ..Default::default()
    }
}

pub(super) struct GatedNamedClear {
    pub(super) inner: crate::named_lease::InMemoryNamedLeaseStore,
    pub(super) gate_once: std::sync::atomic::AtomicBool,
    pub(super) entered: tokio::sync::Notify,
    pub(super) resume: tokio::sync::Notify,
}

impl GatedNamedClear {
    pub(super) async fn gate(&self) {
        if self.gate_once.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
    }
}

#[async_trait::async_trait]
impl NamedLeaseStore for GatedNamedClear {
    async fn set(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
    ) -> Result<(), crate::named_lease::NamedLeaseStoreError> {
        self.inner.set(tenant, name, lease).await
    }
    async fn get(
        &self,
        tenant: &TenantId,
        name: &str,
    ) -> Result<Option<LeaseId>, crate::named_lease::NamedLeaseStoreError> {
        self.inner.get(tenant, name).await
    }
    async fn claim(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<NameClaim, crate::named_lease::NamedLeaseStoreError> {
        self.inner.claim(tenant, name, lease, pending_ttl).await
    }
    async fn replace_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        old: &LeaseId,
        new: &LeaseId,
        pending_ttl: Duration,
    ) -> Result<bool, crate::named_lease::NamedLeaseStoreError> {
        self.inner.replace_if_matches(tenant, name, old, new, pending_ttl).await
    }
    async fn confirm(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
    ) -> Result<bool, crate::named_lease::NamedLeaseStoreError> {
        self.inner.confirm(tenant, name, lease).await
    }
    async fn clear(&self, tenant: &TenantId, name: &str) -> Result<(), crate::named_lease::NamedLeaseStoreError> {
        self.gate().await;
        self.inner.clear(tenant, name).await
    }
    async fn clear_if_matches(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
    ) -> Result<bool, crate::named_lease::NamedLeaseStoreError> {
        self.gate().await;
        self.inner.clear_if_matches(tenant, name, lease).await
    }
}

pub(super) async fn orphan_claim_fixture()
-> anyhow::Result<(BrokerCore, Arc<InMemoryDriveStore>, TenantId, DriveId, LeaseId)> {
    let tenant = TenantId::new("acme")?;
    let drive = DriveId {
        tenant: tenant.clone(),
        name: trogon_sandbox_contract::DriveName::new("scratch")?,
    };
    let lease_id = LeaseId::new("orphan-lease");
    let store = Arc::new(InMemoryDriveStore::new());
    store.create(&tenant, &drive, 1, "bucket".into()).await?;
    store.set_attached_lease(&drive, Some(lease_id.clone())).await?;
    let mut core = make_core(mock_provider_response()?)?;
    core.drive_store = store.clone();
    Ok((core, store, tenant, drive, lease_id))
}

pub(super) async fn claim_holder(store: &InMemoryDriveStore, drive: &DriveId) -> anyhow::Result<Option<LeaseId>> {
    let record = store.get(drive).await?.context("drive record")?;
    Ok(record.attached_lease)
}

pub(super) struct GatedDriveClaim {
    pub(super) inner: Arc<dyn DriveStore>,
    pub(super) entered: tokio::sync::Notify,
    pub(super) resume: tokio::sync::Notify,
    pub(super) committed: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl DriveStore for GatedDriveClaim {
    async fn create(
        &self,
        tenant: &TenantId,
        drive: &DriveId,
        size: u64,
        bucket: String,
    ) -> Result<trogon_sandbox_contract::DriveRecord, DriveStoreError> {
        self.inner.create(tenant, drive, size, bucket).await
    }
    async fn get(&self, drive: &DriveId) -> Result<Option<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
        self.inner.get(drive).await
    }
    async fn list(&self, tenant: &TenantId) -> Result<Vec<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
        self.inner.list(tenant).await
    }
    async fn delete(&self, drive: &DriveId) -> Result<(), DriveStoreError> {
        self.inner.delete(drive).await
    }
    async fn set_attached_lease(&self, drive: &DriveId, lease: Option<LeaseId>) -> Result<(), DriveStoreError> {
        self.entered.notify_one();
        self.resume.notified().await;
        self.inner.set_attached_lease(drive, lease).await?;
        self.committed.notify_one();
        std::future::pending().await
    }
    async fn release_if_attached(&self, drive: &DriveId, lease: &LeaseId) -> Result<bool, DriveStoreError> {
        self.inner.release_if_attached(drive, lease).await
    }
}

pub(super) struct GatedDriveRelease {
    pub(super) inner: Arc<InMemoryDriveStore>,
    pub(super) entered: tokio::sync::Notify,
    pub(super) resume: tokio::sync::Notify,
}

impl GatedDriveRelease {
    pub(super) async fn gate(&self) {
        self.entered.notify_one();
        self.resume.notified().await;
    }
}

#[async_trait::async_trait]
impl DriveStore for GatedDriveRelease {
    async fn create(
        &self,
        tenant: &TenantId,
        drive: &DriveId,
        size: u64,
        bucket: String,
    ) -> Result<trogon_sandbox_contract::DriveRecord, DriveStoreError> {
        self.inner.create(tenant, drive, size, bucket).await
    }
    async fn get(&self, drive: &DriveId) -> Result<Option<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
        self.inner.get(drive).await
    }
    async fn list(&self, tenant: &TenantId) -> Result<Vec<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
        self.inner.list(tenant).await
    }
    async fn delete(&self, drive: &DriveId) -> Result<(), DriveStoreError> {
        self.inner.delete(drive).await
    }
    async fn set_attached_lease(&self, drive: &DriveId, lease: Option<LeaseId>) -> Result<(), DriveStoreError> {
        if lease.is_none() {
            self.gate().await;
        }
        self.inner.set_attached_lease(drive, lease).await
    }
    async fn release_if_attached(&self, drive: &DriveId, lease: &LeaseId) -> Result<bool, DriveStoreError> {
        self.gate().await;
        self.inner.release_if_attached(drive, lease).await
    }
}

pub(super) async fn dispatch_to_worker_with_epoch_support(
    supports_lease_epoch: bool,
) -> anyhow::Result<(serde_json::Value, LeaseRecord)> {
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let mut core = make_core(mock_provider_response()?)?;
    core.bus = bus.clone();
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut heartbeat = worker_heartbeat("epoch-worker", ProviderKind::Fly, default_capability_profile())?;
    heartbeat.supports_lease_epoch = CapabilitySupport::from(supports_lease_epoch);
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    let LeaseResponse::Granted(granted) = core.handle_lease_request(make_request(vec![ProviderKind::Fly])?).await
    else {
        anyhow::bail!("fixture denied");
    };
    let payload = locked(&bus.last_payload)?.clone().context("dispatch payload")?;
    let record = core
        .active_leases
        .get(&granted.lease_id)
        .await?
        .context("lease record")?;
    Ok((serde_json::from_slice(&payload)?, record))
}
