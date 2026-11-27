use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU32, Ordering},
};

use super::*;
use anyhow::Context as _;

use crate::active_lease::InMemoryActiveLeaseStore;
use crate::command_logs::CommandLogStore;
use crate::default_capability_profile::default_capability_profile;
use crate::host_import_policy::InMemoryHostImportPolicy;
use crate::idempotency::InMemoryIdempotencyStore;
use crate::image_policy::{InMemoryImageAllowList, TenantImagePolicy};
use crate::limits::{InMemoryTenantLimits, TenantLimit};
use crate::metrics::BrokerMetrics;
use crate::quota::InMemoryQuotaStore;
use crate::worker_registry::InMemoryWorkerRegistry;

use trogon_sandbox_contract::{
    CapabilitySupport, CommandOutput, CommandOutputChunk, DriveId, DriveMount, DriveMountMode, ExecutionId, LeaseName,
    LeasePersistence, LeasePrewarm, LeaseRequestKey, MountPath, OciImageRef, OperationKind, OutputStream, ProcessSpec,
    ProviderCapabilityProfile, ProviderKind, ReadinessProbe, ReadyPorts, SandboxId, SandboxImage, SandboxSpec,
    SandboxSpecParts, TenantId, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat, WorkerId, WorkspaceSource,
};

/// Result type of a test body.
pub(crate) type TestResult = Result<(), Box<dyn Error>>;

/// Most drives one sandbox mounts.
const MAX_DRIVES_PER_SANDBOX: usize = SandboxSpec::MAX_DRIVES;

const TEST_CPU_SECONDS: u32 = 60;
const TEST_MEMORY_MIB: u32 = 512;

mod dispatch_support;
use dispatch_support::*;

mod lifecycle_support;
use lifecycle_support::*;

mod custody_support;
use custody_support::*;

mod command_support;
use command_support::*;

mod snapshot_support;
use snapshot_support::*;

mod admission_support;
use admission_support::*;

mod operation_support;
use operation_support::*;

/// A heartbeat of worker `worker_id` advertising `profile`, speaking the
/// legacy dispatch protocol, with room for four leases.
pub(crate) fn worker_heartbeat(
    worker_id: &str,
    provider: ProviderKind,
    profile: ProviderCapabilityProfile,
) -> anyhow::Result<WorkerHeartbeat> {
    Ok(WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: WorkerId::new(worker_id)?,
        provider,
        dispatch_protocol: Default::default(),
        native_volume_bindings: Vec::new(),
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases: 0,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: vec![],
        profile,
        warm_pool_population: HashMap::new(),
    })
}

/// Locks `mutex`, reporting a poisoned lock as an error.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> anyhow::Result<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| anyhow::anyhow!("a test lock was poisoned"))
}

/// Parts of a minimal valid spec booting `ubuntu:24.04`.
pub(crate) fn spec_parts() -> anyhow::Result<SandboxSpecParts> {
    Ok(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(TEST_CPU_SECONDS).context("zero cpu seconds")?,
        memory_mib: NonZeroU32::new(TEST_MEMORY_MIB).context("zero memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })
}

/// A minimal valid spec booting `ubuntu:24.04`.
pub(crate) fn make_spec() -> anyhow::Result<SandboxSpec> {
    Ok(SandboxSpec::new(spec_parts()?)?)
}

/// Rebuilds `spec` after `edit` changed its parts.
pub(crate) fn with_spec_parts(
    spec: SandboxSpec,
    edit: impl FnOnce(&mut SandboxSpecParts),
) -> anyhow::Result<SandboxSpec> {
    let mut parts = spec.into_parts();
    edit(&mut parts);
    Ok(SandboxSpec::new(parts)?)
}

/// Rebuilds the spec of `request` after `edit` changed its parts.
pub(crate) fn edit_request_spec(
    request: LeaseRequest,
    edit: impl FnOnce(&mut SandboxSpecParts),
) -> anyhow::Result<LeaseRequest> {
    let mut request = request;
    request.spec = with_spec_parts(request.spec, edit)?;
    Ok(request)
}

struct CountingQuotaStore {
    inner: InMemoryQuotaStore,
    count: Arc<AtomicU32>,
}

impl CountingQuotaStore {
    fn new() -> (Self, Arc<AtomicU32>) {
        let count = Arc::new(AtomicU32::new(0));
        (
            Self {
                inner: InMemoryQuotaStore::new(),
                count: Arc::clone(&count),
            },
            count,
        )
    }
}

#[async_trait::async_trait]
impl QuotaStore for CountingQuotaStore {
    async fn check_and_reserve(
        &self,
        tenant: &TenantId,
        cpu: u32,
        ttl: u32,
        cpu_cap: u32,
        wall_cap: u32,
    ) -> Result<(), LeaseError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.inner.check_and_reserve(tenant, cpu, ttl, cpu_cap, wall_cap).await
    }

    async fn release(&self, tenant: &TenantId, cpu: u32, ttl: u32) -> Result<(), crate::quota::QuotaStoreError> {
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

pub(crate) fn make_request(provider_pref: Vec<ProviderKind>) -> anyhow::Result<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("test-tenant")?,
        provider_pref,
        spec: make_spec()?,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("test-key-1")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::from(false),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::from(true),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

pub(crate) fn provider_lease(sandbox_id: &str, evidence_subject: &str) -> anyhow::Result<CoreProviderLease> {
    Ok(CoreProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new(sandbox_id)?,
        evidence_subject: evidence_subject.to_string(),
        worker_id: None,
        attempt_id: None,
    })
}

pub(crate) fn mock_provider_response() -> anyhow::Result<Bytes> {
    let lease = provider_lease("sandbox-abc", "sandbox.evidence.abc.>")?;
    Ok(Bytes::from(serde_json::to_vec(&lease)?))
}

fn default_pins() -> Arc<crate::digest_pinning::InMemorySnapshotPinStore> {
    crate::digest_pinning::InMemorySnapshotPinStore::new()
}

fn default_config() -> BrokerConfig {
    BrokerConfig {
        default_cpu_cap: 3600,
        default_wall_cap: 7200,
        enforce_image_allowlist: false,
        enforce_worker_registry: false,
        default_max_cpu_per_request: 3600,
        default_max_memory_mib_per_request: 8192,
        default_active_lease_cap: 50,
        snapshot_registry: "snapshots.trogon.internal".to_string(),
        provider_dispatch_timeout: Duration::from_secs(600),
    }
}

fn default_tag_store() -> Arc<dyn crate::snapshot_tag::SnapshotTagStore> {
    let audit: Arc<dyn crate::snapshot_tag::SnapshotTagAuditor> =
        Arc::new(crate::snapshot_tag::InMemorySnapshotTagAuditor::new());
    Arc::new(crate::snapshot_tag::InMemorySnapshotTagStore::new(audit))
}

fn default_resolved_digests() -> Arc<dyn crate::resolved_digest::ResolvedDigestStore> {
    Arc::new(crate::resolved_digest::InMemoryResolvedDigestStore::new())
}

fn default_drive_store() -> Arc<dyn DriveStore> {
    Arc::new(InMemoryDriveStore::new())
}

fn default_named_leases() -> Arc<dyn crate::named_lease::NamedLeaseStore> {
    Arc::new(crate::named_lease::InMemoryNamedLeaseStore::new())
}

/// A [`DriveDataPurger`] test double that records every [`DriveRecord`] it
/// was asked to purge and always succeeds.
#[derive(Default)]
struct RecordingDriveDataPurger {
    purged: Mutex<Vec<trogon_sandbox_contract::DriveRecord>>,
}

impl RecordingDriveDataPurger {
    fn new() -> Self {
        Self::default()
    }

    fn purged_records(&self) -> anyhow::Result<Vec<trogon_sandbox_contract::DriveRecord>> {
        Ok(locked(&self.purged)?.clone())
    }
}

#[async_trait::async_trait]
impl DriveDataPurger for RecordingDriveDataPurger {
    async fn purge(
        &self,
        record: &trogon_sandbox_contract::DriveRecord,
    ) -> Result<(), crate::drive_purge::DrivePurgeError> {
        self.purged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(record.clone());
        Ok(())
    }
}

/// A [`DriveDataPurger`] test double that always fails, used to verify
/// `handle_delete_drive` denies cleanly (fail-closed) without removing the
/// drive's KV record when storage purge fails.
struct FailingDriveDataPurger;

#[async_trait::async_trait]
impl DriveDataPurger for FailingDriveDataPurger {
    async fn purge(
        &self,
        _record: &trogon_sandbox_contract::DriveRecord,
    ) -> Result<(), crate::drive_purge::DrivePurgeError> {
        Err(crate::drive_purge::DrivePurgeError::ProviderOwned)
    }
}

/// Every injection point a test core varies.
struct CoreParts {
    bus: Arc<dyn crate::bus::BrokerBus>,
    quota: Arc<dyn QuotaStore>,
    image_allowlist: Arc<dyn crate::image_policy::ImageAllowList>,
    tenant_limits: Arc<dyn crate::limits::TenantLimits>,
    worker_registry: Arc<dyn crate::worker_registry::WorkerRegistry>,
    drive_store: Arc<dyn DriveStore>,
    config: BrokerConfig,
}

impl CoreParts {
    fn new(bus: Arc<dyn crate::bus::BrokerBus>) -> Self {
        Self {
            bus,
            quota: Arc::new(InMemoryQuotaStore::new()),
            image_allowlist: Arc::new(InMemoryImageAllowList::new()),
            tenant_limits: Arc::new(InMemoryTenantLimits::new()),
            worker_registry: InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
            drive_store: default_drive_store(),
            config: default_config(),
        }
    }

    fn build(self) -> anyhow::Result<BrokerCore> {
        Ok(BrokerCore::new(
            self.bus,
            self.quota,
            Arc::new(InMemoryIdempotencyStore::new()),
            Arc::new(InMemoryActiveLeaseStore::new()),
            self.image_allowlist,
            Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
            Arc::new(InMemoryHostImportPolicy::new()),
            self.tenant_limits,
            self.worker_registry,
            default_pins(),
            default_tag_store(),
            default_resolved_digests(),
            self.drive_store,
            default_named_leases(),
            self.config,
            BrokerMetrics::new()?,
        ))
    }
}

fn make_core(provider_response: Bytes) -> anyhow::Result<BrokerCore> {
    CoreParts::new(Arc::new(MockBus { provider_response })).build()
}

fn make_core_with_drive_store(
    provider_response: Bytes,
    drive_store: Arc<dyn DriveStore>,
) -> anyhow::Result<BrokerCore> {
    CoreParts {
        drive_store,
        ..CoreParts::new(Arc::new(MockBus { provider_response }))
    }
    .build()
}

fn make_core_with_counting_quota(provider_response: Bytes) -> anyhow::Result<(BrokerCore, Arc<AtomicU32>)> {
    let (quota, count) = CountingQuotaStore::new();
    let core = CoreParts {
        quota: Arc::new(quota),
        ..CoreParts::new(Arc::new(MockBus { provider_response }))
    }
    .build()?;
    Ok((core, count))
}

fn checkpoint_receipt_digest() -> anyhow::Result<trogon_sandbox_contract::Digest> {
    Ok(trogon_sandbox_contract::Digest::new(format!(
        "sha256:{}",
        "c".repeat(64)
    ))?)
}

/// A bus that returns a different canned response depending on whether the
/// requested subject is a checkpoint_running RPC or a provider dispatch,
/// so fork tests can control each leg of `handle_fork_request` independently.
fn checkpoint_ok_response() -> anyhow::Result<Bytes> {
    let outcome = SnapshotExportOutcome::Ok(SnapshotReceipt {
        digest: checkpoint_receipt_digest()?,
        bytes: 1024,
        artifact_type: trogon_sandbox_contract::ArtifactType::SandboxRootfsV1,
    });
    Ok(Bytes::from(serde_json::to_vec(&outcome)?))
}

fn checkpoint_err_response() -> anyhow::Result<Bytes> {
    let outcome = SnapshotExportOutcome::Err(trogon_sandbox_contract::SnapshotExportError::Transient {
        message: "worker busy".to_string(),
    });
    Ok(Bytes::from(serde_json::to_vec(&outcome)?))
}

fn network_policy_applied_response() -> anyhow::Result<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(&NetworkPolicyUpdateResponse::Applied)?))
}

fn network_policy_worker_denied_response() -> anyhow::Result<Bytes> {
    let response = NetworkPolicyUpdateResponse::Denied(LeaseError::Backend {
        message: "sidecar unreachable".to_string(),
    });
    Ok(Bytes::from(serde_json::to_vec(&response)?))
}

fn make_core_with_bus(bus: Arc<dyn crate::bus::BrokerBus>) -> anyhow::Result<BrokerCore> {
    CoreParts::new(bus).build()
}

fn running_source_record(tenant: &TenantId, lease_id: &LeaseId) -> anyhow::Result<LeaseRecord> {
    let spec = with_spec_parts(make_spec()?, |parts| {
        parts.snapshot_out = Some(trogon_sandbox_contract::SnapshotPolicy {
            artifact_type: trogon_sandbox_contract::ArtifactType::SandboxRootfsV1,
            target_repository: "snapshots/test-tenant".to_string(),
            retention: None,
            prewarm: None,
            tag_name: None,
        });
    })?;
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: lease_id.clone(),
        tenant: tenant.clone(),
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(300),
        spec,
        sandbox_id: "sandbox-source".to_string(),
        cpu_seconds_consumed: 0,
        wall_seconds_consumed: 0,
        correlation_id: None,
        tags: SandboxTags::default(),
        name: None,
        worker_id: None,
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: None,
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

fn lease_record_at(
    tenant: &TenantId,
    lease_id: &LeaseId,
    status: LeaseStatus,
    created_at: SystemTime,
) -> anyhow::Result<LeaseRecord> {
    let mut record = running_source_record(tenant, lease_id)?;
    record.status = status;
    record.created_at = created_at;
    Ok(record)
}

fn make_fork_request(tenant: &TenantId, source_lease_id: &LeaseId) -> LeaseForkRequest {
    LeaseForkRequest {
        tenant: tenant.clone(),
        source_lease_id: source_lease_id.clone(),
        ttl_seconds: 300,
        idempotency_key: "fork-key-1".to_string(),
    }
}

/// Counts checkpoint RPCs while serving the same canned responses as
/// [`RoutingBus`].
struct CountingForkBus {
    checkpoints: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for CountingForkBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains("checkpoint_running") {
            self.checkpoints.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            checkpoint_ok_response()
        } else {
            mock_provider_response()
        }
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

fn unretained_source_record(
    tenant: &TenantId,
    lease_id: &LeaseId,
    retention: trogon_sandbox_contract::RetentionToggles,
) -> anyhow::Result<LeaseRecord> {
    let mut record = running_source_record(tenant, lease_id)?;
    record.spec = with_spec_parts(record.spec, |parts| {
        parts.snapshot_out = None;
        parts.retention = retention;
    })?;
    Ok(record)
}

async fn release_with_logs(retention: trogon_sandbox_contract::RetentionToggles) -> anyhow::Result<bool> {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let other = LeaseId::new("lease-other");
    let execution_id = ExecutionId::new("exec-1");
    let store = Arc::new(crate::command_logs::InMemoryCommandLogStore::new(
        crate::command_logs::CommandLogRetention::default(),
    ));
    for lease in [&lease_id, &other] {
        store.append(
            lease,
            CommandOutputChunk {
                execution_id: execution_id.clone(),
                stream: OutputStream::Stdout,
                seq: 0,
                data: b"out".to_vec(),
            },
        );
    }
    let core = make_core(mock_provider_response()?)?.with_command_logs(store.clone());
    core.active_leases
        .put(&unretained_source_record(&tenant, &lease_id, retention)?)
        .await?;
    core.release_lease(lease_id.clone(), CancelIntent::Release, LeaseReleasedReason::Cancelled)
        .await?;

    let other_page = store.read(&other, &execution_id, 0, 1024).await?;
    assert_eq!(other_page.chunks.len(), 1, "other leases keep their output");
    let page = store.read(&lease_id, &execution_id, 0, 1024).await?;
    Ok(!page.chunks.is_empty())
}

struct FixedRemover(crate::snapshot_tracker::SnapshotRemoval);

#[async_trait::async_trait]
impl crate::snapshot_tracker::SnapshotRemover for FixedRemover {
    async fn remove(
        &self,
        _tenant: &TenantId,
        _digest: &Digest,
    ) -> Result<crate::snapshot_tracker::SnapshotRemoval, crate::snapshot_tracker::SnapshotRemovalError> {
        Ok(self.0.clone())
    }
}

fn make_network_policy_update_request(
    lease_id: &LeaseId,
    network_policy: trogon_sandbox_contract::NetworkPolicy,
) -> NetworkPolicyUpdateRequest {
    NetworkPolicyUpdateRequest {
        lease_id: lease_id.clone(),
        network_policy,
    }
}

fn make_core_enforcing_empty_registry(provider_response: Bytes) -> anyhow::Result<BrokerCore> {
    let mut config = default_config();
    config.enforce_worker_registry = true;
    CoreParts {
        worker_registry: InMemoryWorkerRegistry::new(std::time::Duration::from_secs(30)),
        config,
        ..CoreParts::new(Arc::new(MockBus { provider_response }))
    }
    .build()
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod entity_shadow_mode;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod credential_policy_flow;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod entity_intent_flow;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod reconcile_flow;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod rollback_flow;

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod command_launch_cancellation;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod dispatch_recovery;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod dormancy_flow;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod drive_admission;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod drive_claims;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod drive_dispatch;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod durable_operations;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod fork_and_snapshot;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod host_mounts;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod idle_policy;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod lease_admission;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod lease_usage;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod lease_verbs;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod lifetime_and_reap;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod logs_stdin_files;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod mounts_and_network_policy;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod named_leases;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod native_passthrough;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod native_snapshot_delete;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod operation_cancellation;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod pause_resume;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod release_and_purge;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod run_command;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod snapshot_formats;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod snapshot_placement;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod snapshot_tree;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod substrate_admission;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod tags_extend_drives;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod tenant_limits;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod terminal_and_profile_gates;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod worker_replies_and_delete;
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod worker_requirements;
