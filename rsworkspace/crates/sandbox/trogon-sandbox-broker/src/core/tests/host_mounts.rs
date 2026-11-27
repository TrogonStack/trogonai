use super::*;
use crate::host_mount_policy::{InMemoryHostMountAllowList, TenantHostMountPolicy};
use std::time::Duration;

struct TrackingQuota {
    inner: Arc<InMemoryQuotaStore>,
    released: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl QuotaStore for TrackingQuota {
    async fn check_and_reserve(
        &self,
        tenant: &TenantId,
        cpu: u32,
        ttl: u32,
        cpu_cap: u32,
        wall_cap: u32,
    ) -> Result<(), LeaseError> {
        self.inner.check_and_reserve(tenant, cpu, ttl, cpu_cap, wall_cap).await
    }

    async fn release(&self, tenant: &TenantId, cpu: u32, ttl: u32) -> Result<(), crate::quota::QuotaStoreError> {
        self.released.fetch_add(1, Ordering::SeqCst);
        self.inner.release(tenant, cpu, ttl).await
    }

    async fn active_lease_count(&self, tenant: &TenantId) -> Result<u32, crate::quota::QuotaStoreError> {
        self.inner.active_lease_count(tenant).await
    }

    async fn check_active_lease_cap(&self, tenant: &TenantId) -> Result<(), LeaseError> {
        self.inner.check_active_lease_cap(tenant).await
    }

    async fn increment_active_leases(&self, tenant: &TenantId) -> Result<(), crate::quota::QuotaStoreError> {
        self.inner.increment_active_leases(tenant).await
    }

    async fn decrement_active_leases(&self, tenant: &TenantId) -> Result<(), crate::quota::QuotaStoreError> {
        self.inner.decrement_active_leases(tenant).await
    }

    async fn record_snapshot_published(
        &self,
        tenant: &TenantId,
        bytes: u64,
        default_snapshot: crate::quota::SnapshotQuota,
    ) -> Result<(), LeaseError> {
        self.inner
            .record_snapshot_published(tenant, bytes, default_snapshot)
            .await
    }

    async fn record_snapshot_deleted(
        &self,
        tenant: &TenantId,
        bytes: u64,
    ) -> Result<(), crate::quota::QuotaStoreError> {
        self.inner.record_snapshot_deleted(tenant, bytes).await
    }

    async fn check_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), LeaseError> {
        self.inner.check_prewarm_dispatch(tenant, cpu_seconds).await
    }

    async fn record_prewarm_dispatch(
        &self,
        tenant: &TenantId,
        cpu_seconds: u32,
    ) -> Result<(), crate::quota::QuotaStoreError> {
        self.inner.record_prewarm_dispatch(tenant, cpu_seconds).await
    }

    async fn record_prewarm_release(
        &self,
        tenant: &TenantId,
        cpu_seconds: u32,
    ) -> Result<(), crate::quota::QuotaStoreError> {
        self.inner.record_prewarm_release(tenant, cpu_seconds).await
    }
}

/// A core whose tenant may mount host paths under `/srv/workspaces`.
async fn host_mount_core(parts: CoreParts) -> anyhow::Result<BrokerCore> {
    let allow_list = Arc::new(InMemoryHostMountAllowList::new());
    allow_list
        .insert(
            &TenantId::new("test-tenant")?,
            TenantHostMountPolicy {
                path_prefixes: vec!["/srv/workspaces".to_string()],
            },
        )
        .await;
    let mut core = parts.build()?;
    core.host_mount_allowlist = allow_list;
    Ok(core)
}

#[tokio::test]
async fn host_mount_tenant_with_no_policy_is_denied() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_host_mount_request(vec![ProviderKind::Mock], "/srv/workspaces/acme/repo")?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected InvalidSpec, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn host_mount_under_allowed_prefix_is_granted() -> TestResult {
    let core = host_mount_core(CoreParts::new(Arc::new(MockBus {
        provider_response: mock_provider_response()?,
    })))
    .await?;

    let req = make_host_mount_request(vec![ProviderKind::Mock], "/srv/workspaces/acme/repo")?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Granted(_)),
        "expected Granted, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn host_mount_outside_allowed_prefix_is_denied() -> TestResult {
    let core = host_mount_core(CoreParts::new(Arc::new(MockBus {
        provider_response: mock_provider_response()?,
    })))
    .await?;

    let req = make_host_mount_request(vec![ProviderKind::Mock], "/etc/passwd")?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected InvalidSpec, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn host_mount_on_k8s_microvm_is_denied_with_quota_refund() -> TestResult {
    let released = Arc::new(AtomicU32::new(0));
    let tracking_quota = Arc::new(TrackingQuota {
        inner: Arc::new(InMemoryQuotaStore::new()),
        released: Arc::clone(&released),
    });
    let core = host_mount_core(CoreParts {
        quota: tracking_quota,
        worker_registry: InMemoryWorkerRegistry::new(Duration::from_secs(30)),
        ..CoreParts::new(Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }))
    })
    .await?;

    let req = make_host_mount_request(vec![ProviderKind::K8sMicroVm], "/srv/workspaces/acme/repo")?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected InvalidSpec for k8s_microvm + host_mount, got {response:?}"
    );
    assert_eq!(
        released.load(Ordering::SeqCst),
        1,
        "quota must be refunded when k8s_microvm rejects host_mount"
    );
    Ok(())
}

#[tokio::test]
async fn host_mount_on_daytona_is_denied_with_quota_refund() -> TestResult {
    let released = Arc::new(AtomicU32::new(0));
    let tracking_quota = Arc::new(TrackingQuota {
        inner: Arc::new(InMemoryQuotaStore::new()),
        released: Arc::clone(&released),
    });
    let core = host_mount_core(CoreParts {
        quota: tracking_quota,
        worker_registry: InMemoryWorkerRegistry::new(Duration::from_secs(30)),
        ..CoreParts::new(Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }))
    })
    .await?;

    let req = make_host_mount_request(vec![ProviderKind::Daytona], "/srv/workspaces/acme/repo")?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected InvalidSpec for daytona + host_mount, got {response:?}"
    );
    assert_eq!(
        released.load(Ordering::SeqCst),
        1,
        "quota must be refunded when daytona rejects host_mount"
    );
    Ok(())
}
