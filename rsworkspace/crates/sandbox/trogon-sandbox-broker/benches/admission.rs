//! Broker admission cost: everything `handle_lease_request` does between a request arriving and a worker being asked
//! for the lease, with the worker replaced by a bus that answers immediately. No NATS server or Docker is needed.
//!
//! The figures are for comparing two builds on the same machine, not for gating CI. See
//! `docs/how-to/sandbox/measure-latency.md`.
//!
//! ```text
//! cargo bench -p trogon-sandbox-broker --bench admission
//! ```

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use criterion::Criterion;
use trogon_sandbox_broker::active_lease::InMemoryActiveLeaseStore;
use trogon_sandbox_broker::bus::BrokerBus;
use trogon_sandbox_broker::core::{BrokerConfig, BrokerCore, InMemoryDriveStore};
use trogon_sandbox_broker::digest_pinning::InMemorySnapshotPinStore;
use trogon_sandbox_broker::host_import_policy::InMemoryHostImportPolicy;
use trogon_sandbox_broker::host_mount_policy::InMemoryHostMountAllowList;
use trogon_sandbox_broker::idempotency::InMemoryIdempotencyStore;
use trogon_sandbox_broker::image_policy::InMemoryImageAllowList;
use trogon_sandbox_broker::limits::InMemoryTenantLimits;
use trogon_sandbox_broker::metrics::BrokerMetrics;
use trogon_sandbox_broker::named_lease::InMemoryNamedLeaseStore;
use trogon_sandbox_broker::quota::InMemoryQuotaStore;
use trogon_sandbox_broker::resolved_digest::InMemoryResolvedDigestStore;
use trogon_sandbox_broker::snapshot_tag::{InMemorySnapshotTagAuditor, InMemorySnapshotTagStore, SnapshotTagAuditor};
use trogon_sandbox_broker::worker_registry::InMemoryWorkerRegistry;
use trogon_sandbox_contract::{
    DeleteLeaseRequest, DeleteLeaseResponse, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey,
    LeaseResponse, OciImageRef, ProviderKind, ProviderLease, SandboxId, SandboxImage, SandboxSpec, SandboxSpecParts,
    SandboxTags, TenantId, WorkspaceSource,
};

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

/// A bus whose every request is answered at once with a fixed provider lease.
struct InstantWorker {
    reply: Bytes,
}

#[async_trait::async_trait]
impl BrokerBus for InstantWorker {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(self.reply.clone())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

fn broker() -> BenchResult<BrokerCore> {
    let lease = ProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new("sandbox-bench")?,
        evidence_subject: "sandbox.evidence.bench.>".to_string(),
        worker_id: None,
        attempt_id: None,
    };
    let audit: Arc<dyn SnapshotTagAuditor> = Arc::new(InMemorySnapshotTagAuditor::new());
    Ok(BrokerCore::new(
        Arc::new(InstantWorker {
            reply: Bytes::from(serde_json::to_vec(&lease)?),
        }),
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        InMemorySnapshotPinStore::new(),
        Arc::new(InMemorySnapshotTagStore::new(audit)),
        Arc::new(InMemoryResolvedDigestStore::new()),
        Arc::new(InMemoryDriveStore::new()),
        Arc::new(InMemoryNamedLeaseStore::new()),
        BrokerConfig {
            default_cpu_cap: u32::MAX,
            default_wall_cap: u32::MAX,
            enforce_image_allowlist: false,
            enforce_worker_registry: false,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 50,
            snapshot_registry: "snapshots.trogon.internal".to_string(),
            provider_dispatch_timeout: Duration::from_secs(600),
        },
        BrokerMetrics::new()?,
    ))
}

fn tenant() -> BenchResult<TenantId> {
    Ok(TenantId::new("bench-tenant")?)
}

fn spec() -> BenchResult<SandboxSpec> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(1).ok_or("zero cpu seconds")?,
        memory_mib: NonZeroU32::new(512).ok_or("zero memory")?,
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
    })?)
}

fn request(n: u64) -> BenchResult<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: tenant()?,
        provider_pref: vec![ProviderKind::Mock],
        spec: spec()?,
        ttl_seconds: 300,
        // A fresh key per iteration, so idempotency replay never short-circuits.
        idempotency_key: LeaseRequestKey::new(format!("bench-{n}"))?,
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

/// Admits one lease under a fresh key, then deletes it.
async fn grant_and_delete(core: &BrokerCore, n: u64) -> BenchResult<()> {
    let response = core.handle_lease_request(request(n)?).await;
    let LeaseResponse::Granted(granted) = response else {
        return Err(format!("not granted: {response:?}").into());
    };
    let deleted = core
        .handle_delete_lease(DeleteLeaseRequest {
            tenant: tenant()?,
            lease_id: granted.lease_id,
            operation_id: None,
        })
        .await;
    if deleted != (DeleteLeaseResponse::Deleted { operation_id: None }) {
        return Err(format!("not deleted: {deleted:?}").into());
    }
    Ok(())
}

/// Sends the request that `n` keys, which the broker answers from its idempotency record after the first.
async fn replay(core: &BrokerCore, n: u64) -> BenchResult<()> {
    let response = core.handle_lease_request(request(n)?).await;
    if !matches!(response, LeaseResponse::Granted(_)) {
        return Err(format!("not granted: {response:?}").into());
    }
    Ok(())
}

/// Reports the first failure an iteration recorded, since a bench closure cannot return one.
fn first_failure(failures: &RefCell<Vec<String>>) -> BenchResult<()> {
    match failures.borrow().first() {
        Some(failure) => Err(failure.clone().into()),
        None => Ok(()),
    }
}

fn admission(c: &mut Criterion) -> BenchResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let core = broker()?;
    let next = AtomicU64::new(0);
    let failures = RefCell::new(Vec::new());

    // The in-memory quota store caps live leases at 50 and offers no way to raise it, so each iteration deletes the
    // lease it was granted. The figure therefore covers admission plus release.
    c.bench_function("grant and delete a lease", |b| {
        b.to_async(&runtime).iter(|| async {
            if let Err(error) = grant_and_delete(&core, next.fetch_add(1, Ordering::Relaxed)).await {
                failures.borrow_mut().push(error.to_string());
            }
        });
    });
    first_failure(&failures)?;

    c.bench_function("replay a lease request by idempotency key", |b| {
        if let Err(error) = runtime.block_on(replay(&core, u64::MAX)) {
            failures.borrow_mut().push(error.to_string());
        }
        b.to_async(&runtime).iter(|| async {
            if let Err(error) = replay(&core, u64::MAX).await {
                failures.borrow_mut().push(error.to_string());
            }
        });
    });
    first_failure(&failures)
}

fn main() -> BenchResult<()> {
    let mut criterion = Criterion::default().configure_from_args();
    admission(&mut criterion)?;
    criterion.final_summary();
    Ok(())
}
