use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_broker::active_lease::InMemoryActiveLeaseStore;
use trogon_sandbox_broker::bus::BrokerBus;
use trogon_sandbox_broker::core::{BrokerConfig, BrokerCore};
use trogon_sandbox_broker::digest_pinning::InMemorySnapshotPinStore;
use trogon_sandbox_broker::host_import_policy::InMemoryHostImportPolicy;
use trogon_sandbox_broker::idempotency::InMemoryIdempotencyStore;
use trogon_sandbox_broker::image_policy::InMemoryImageAllowList;
use trogon_sandbox_broker::limits::InMemoryTenantLimits;
use trogon_sandbox_broker::metrics::BrokerMetrics;
use trogon_sandbox_broker::prewarm::{
    InMemoryPrewarmStore, PrewarmController, PrewarmState, PrewarmStore, build_prewarm_request,
};
use trogon_sandbox_broker::quota::{InMemoryQuotaStore, PrewarmQuota, QuotaStore};
use trogon_sandbox_broker::resolved_digest::InMemoryResolvedDigestStore;
use trogon_sandbox_broker::snapshot_tag::{InMemorySnapshotTagAuditor, InMemorySnapshotTagStore};
use trogon_sandbox_broker::snapshot_tracker::{InMemoryLiveSnapshotStore, LiveSnapshotStore};
use trogon_sandbox_broker::worker_registry::InMemoryWorkerRegistry;
use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, Digest, LeaseError, LeaseResponse, LiveSnapshotEntry, PrewarmSpec, ProviderKind,
    ProviderLease, SandboxId, TenantId, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat, WorkerId,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn sample_digest() -> TestResult<Digest> {
    Ok(Digest::new(format!("sha256:{}", "a".repeat(64)))?)
}

struct CountingBus {
    dispatch_count: Arc<AtomicU32>,
}

#[async_trait]
impl BrokerBus for CountingBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains("local_docker") {
            self.dispatch_count.fetch_add(1, Ordering::SeqCst);
            let lease = ProviderLease {
                epoch: None,
                sandbox_id: SandboxId::new("warm-sandbox")?,
                evidence_subject: "sandbox.evidence.warm.>".to_string(),
                worker_id: None,
                attempt_id: None,
            };
            return Ok(Bytes::from(serde_json::to_vec(&lease)?));
        }
        Ok(Bytes::new())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

fn sample_tenant() -> TestResult<TenantId> {
    Ok(TenantId::new("prewarm-tenant")?)
}

async fn seed_live_snapshot(live: &InMemoryLiveSnapshotStore, tenant: &TenantId, digest: &Digest) -> TestResult {
    LiveSnapshotStore::put(
        live,
        digest,
        &LiveSnapshotEntry {
            tenant: tenant.clone(),
            artifact_type: ArtifactType::SandboxRootfsV1,
            published_at: SystemTime::now(),
            bytes: 1024,
            generation: None,
        },
    )
    .await?;
    Ok(())
}

async fn seed_prewarm_policy(
    store: &InMemoryPrewarmStore,
    tenant: &TenantId,
    digest: &Digest,
    pool_size: u32,
) -> TestResult {
    PrewarmStore::put(
        store,
        tenant,
        digest,
        &PrewarmState::new(
            tenant.clone(),
            digest.clone(),
            PrewarmSpec {
                pool_size,
                providers: vec![ProviderKind::LocalDocker],
            },
        ),
    )
    .await?;
    Ok(())
}

async fn inject_worker(registry: &InMemoryWorkerRegistry, digest: &Digest, warm_count: u32) -> TestResult {
    let mut warm_pool_population = HashMap::new();
    warm_pool_population.insert(digest.clone(), warm_count);
    let mut profile = trogon_sandbox_broker::default_capability_profile::default_capability_profile();
    profile.provider = ProviderKind::LocalDocker;
    profile.snapshot.can_restore = CapabilitySupport::Supported;
    profile.lifecycle.supports_prewarm_per_tenant = CapabilitySupport::Supported;
    registry
        .inject(WorkerHeartbeat {
            supports_lease_epoch: CapabilitySupport::Unsupported,
            worker_id: WorkerId::new("docker-1")?,
            provider: ProviderKind::LocalDocker,
            capacity: WorkerCapacity {
                max_concurrent_leases: 8,
                current_leases: 0,
                capabilities: WorkerCapabilities::from(&profile),
            },
            emitted_at: SystemTime::now(),
            cached_digests: vec![digest.clone()],
            profile,
            warm_pool_population,
            dispatch_protocol: Default::default(),
            native_volume_bindings: Default::default(),
        })
        .await;
    Ok(())
}

fn build_controller(
    dispatch_count: Arc<AtomicU32>,
    quota: Arc<InMemoryQuotaStore>,
    registry: Arc<InMemoryWorkerRegistry>,
    live: Arc<InMemoryLiveSnapshotStore>,
    prewarm_store: Arc<InMemoryPrewarmStore>,
) -> TestResult<PrewarmController> {
    let registry_dyn: Arc<dyn trogon_sandbox_broker::worker_registry::WorkerRegistry> = registry;
    let broker = Arc::new(BrokerCore::new(
        Arc::new(CountingBus { dispatch_count }),
        quota,
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(trogon_sandbox_broker::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        Arc::clone(&registry_dyn),
        InMemorySnapshotPinStore::new(),
        Arc::new(InMemorySnapshotTagStore::new(Arc::new(
            InMemorySnapshotTagAuditor::new(),
        ))),
        Arc::new(InMemoryResolvedDigestStore::new()),
        Arc::new(trogon_sandbox_broker::core::InMemoryDriveStore::new()),
        Arc::new(trogon_sandbox_broker::named_lease::InMemoryNamedLeaseStore::new()),
        BrokerConfig {
            default_cpu_cap: 3600,
            default_wall_cap: 7200,
            enforce_image_allowlist: false,
            enforce_worker_registry: false,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 50,
            snapshot_registry: "snapshots.local".into(),
            provider_dispatch_timeout: Duration::from_secs(600),
        },
        BrokerMetrics::new()?,
    ));
    let live_dyn: Arc<dyn LiveSnapshotStore> = live;
    let prewarm_dyn: Arc<dyn PrewarmStore> = prewarm_store;
    Ok(PrewarmController::new(
        broker,
        prewarm_dyn,
        live_dyn,
        registry_dyn,
        "snapshots.local".into(),
    ))
}

#[tokio::test]
async fn prewarm_lease_request_is_granted_by_broker() -> TestResult {
    let tenant = sample_tenant()?;
    let digest = sample_digest()?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_worker(&registry, &digest, 0).await?;

    let dispatch_count = Arc::new(AtomicU32::new(0));
    let quota = Arc::new(InMemoryQuotaStore::new());
    let registry_dyn: Arc<dyn trogon_sandbox_broker::worker_registry::WorkerRegistry> = registry;
    let broker = Arc::new(BrokerCore::new(
        Arc::new(CountingBus {
            dispatch_count: Arc::clone(&dispatch_count),
        }),
        quota,
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(trogon_sandbox_broker::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        registry_dyn,
        InMemorySnapshotPinStore::new(),
        Arc::new(InMemorySnapshotTagStore::new(Arc::new(
            InMemorySnapshotTagAuditor::new(),
        ))),
        Arc::new(InMemoryResolvedDigestStore::new()),
        Arc::new(trogon_sandbox_broker::core::InMemoryDriveStore::new()),
        Arc::new(trogon_sandbox_broker::named_lease::InMemoryNamedLeaseStore::new()),
        BrokerConfig {
            default_cpu_cap: 3600,
            default_wall_cap: 7200,
            enforce_image_allowlist: false,
            enforce_worker_registry: false,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 50,
            snapshot_registry: "snapshots.local".into(),
            provider_dispatch_timeout: Duration::from_secs(600),
        },
        BrokerMetrics::new()?,
    ));
    let req = build_prewarm_request(
        &tenant,
        &digest,
        ArtifactType::SandboxRootfsV1,
        ProviderKind::LocalDocker,
        "snapshots.local",
        &trogon_sandbox_protocol::snapshot_repository_for_tenant(&tenant),
    )?;
    let resp = broker.handle_lease_request(req).await;
    assert!(
        matches!(resp, LeaseResponse::Granted(_)),
        "expected grant, got {resp:?}"
    );
    assert_eq!(dispatch_count.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn prewarm_controller_dispatches_to_target_pool_size() -> TestResult {
    let tenant = sample_tenant()?;
    let digest = sample_digest()?;
    let live = InMemoryLiveSnapshotStore::new();
    seed_live_snapshot(&live, &tenant, &digest).await?;

    let prewarm_store = InMemoryPrewarmStore::new();
    seed_prewarm_policy(&prewarm_store, &tenant, &digest, 3).await?;

    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_worker(&registry, &digest, 0).await?;

    let dispatch_count = Arc::new(AtomicU32::new(0));
    let quota = Arc::new(InMemoryQuotaStore::new());
    let controller = build_controller(
        Arc::clone(&dispatch_count),
        Arc::clone(&quota),
        Arc::clone(&registry),
        Arc::clone(&live),
        Arc::clone(&prewarm_store),
    )?;

    let dispatched = controller.tick().await?;
    assert_eq!(dispatched, 3);
    assert_eq!(dispatch_count.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn prewarm_quota_stops_dispatches_at_cap() -> TestResult {
    let tenant = sample_tenant()?;
    let digest = sample_digest()?;
    let live = InMemoryLiveSnapshotStore::new();
    seed_live_snapshot(&live, &tenant, &digest).await?;

    let prewarm_store = InMemoryPrewarmStore::new();
    seed_prewarm_policy(&prewarm_store, &tenant, &digest, 5).await?;

    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_worker(&registry, &digest, 0).await?;

    let quota = Arc::new(InMemoryQuotaStore::new());
    quota
        .seed_prewarm_quota(
            &tenant,
            PrewarmQuota {
                active_count: 2,
                count_cap: 2,
                total_cpu_seconds: 0,
                cpu_cap: 3600,
            },
        )
        .await;

    let dispatch_count = Arc::new(AtomicU32::new(0));
    let controller = build_controller(
        Arc::clone(&dispatch_count),
        Arc::clone(&quota),
        Arc::clone(&registry),
        Arc::clone(&live),
        Arc::clone(&prewarm_store),
    )?;

    let dispatched = controller.tick().await?;
    assert_eq!(dispatched, 0);
    assert_eq!(dispatch_count.load(Ordering::SeqCst), 0);

    let denied = quota.check_prewarm_dispatch(&tenant, 60).await;
    assert!(matches!(denied, Err(LeaseError::PrewarmQuotaExceeded { .. })));
    Ok(())
}
