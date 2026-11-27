//! Fixtures for the provider tests: a provider over a [`FakeCluster`] with a scripted snapshot backend and guest
//! probe, and lease requests of each image kind.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use kube::core::DynamicObject;
use tempfile::TempDir;
use trogon_sandbox_contract::{
    BootMode, LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, LeaseTtl, OciArtifactRef,
    OciImageRef, PlatformOptions, PrivilegedAccess, ProviderKind, RetentionToggles, SandboxImage, SandboxSpec,
    SandboxSpecParts, SandboxTags, SecretBindings, SnapshotReceipt, TenantId, WorkspaceSource,
};
use trogon_sandbox_kubevirt_worker::{
    GuestProbe, KubeVirtConfig, KubeVirtOps, KubeVirtProvider, KubeVirtProviderError, SnapshotBackend, WarmDiskSource,
    WarmDiskSourceError,
};
use trogon_sandbox_protocol::ProviderError;
use trogon_sandbox_volumes::VolumeClientConfig;

use crate::fake_cluster::FakeCluster;
use crate::support::{self, Boxed, ScriptedDiskSource, TestResult};

type Failure = Box<dyn Error + Send + Sync>;

/// A snapshot backend whose exports succeed or fail on command and which records what it was asked.
pub struct Backend {
    disks: ScriptedDiskSource,
    fail_exports: bool,
    stop_on_checkpoint: Option<Arc<FakeCluster>>,
    calls: Mutex<Vec<String>>,
}

impl Backend {
    pub fn new() -> Self {
        Self {
            disks: ScriptedDiskSource::default(),
            fail_exports: false,
            stop_on_checkpoint: None,
            calls: Mutex::default(),
        }
    }

    pub fn failing_disks() -> Self {
        Self {
            disks: ScriptedDiskSource::failing(),
            ..Self::new()
        }
    }

    pub fn failing_exports() -> Self {
        Self {
            fail_exports: true,
            ..Self::new()
        }
    }

    /// A backend whose checkpoint leaves the VM `name` of `cluster` stopped.
    pub fn stopping(cluster: Arc<FakeCluster>) -> Self {
        Self {
            stop_on_checkpoint: Some(cluster),
            ..Self::new()
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn requested(&self) -> Vec<String> {
        self.disks.requested()
    }

    fn record(&self, call: &str, vm_name: &str, target: &OciArtifactRef) -> Result<SnapshotReceipt, Failure> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{call} {vm_name}"));
        if self.fail_exports {
            return Err("the export failed".into());
        }
        Ok(SnapshotReceipt {
            digest: target.digest.clone(),
            bytes: 7,
            artifact_type: target.artifact_type,
        })
    }
}

#[async_trait]
impl WarmDiskSource for Backend {
    async fn data_volume(
        &self,
        ops: &KubeVirtOps,
        volume: &VolumeClientConfig,
        reference: &OciArtifactRef,
        dv_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<DynamicObject, WarmDiskSourceError> {
        self.disks.data_volume(ops, volume, reference, dv_name, labels).await
    }
}

#[async_trait]
impl SnapshotBackend for Backend {
    async fn export(
        &self,
        _ops: &KubeVirtOps,
        _lease_id: &LeaseId,
        vm_name: &str,
        _labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, Failure> {
        self.record("export", vm_name, target)
    }

    async fn checkpoint(
        &self,
        _ops: &KubeVirtOps,
        _lease_id: &LeaseId,
        vm_name: &str,
        _labels: BTreeMap<String, String>,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, Failure> {
        if let Some(cluster) = &self.stop_on_checkpoint {
            cluster.with_store(|store| {
                store.delete("virtualmachineinstances", vm_name);
            });
        }
        self.record("checkpoint", vm_name, target)
    }
}

/// A guest probe that answers as told and records the VMs it probed.
#[derive(Default)]
pub struct Probe {
    fail: bool,
    probed: Mutex<Vec<String>>,
}

impl Probe {
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    pub fn probed(&self) -> Vec<String> {
        self.probed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl GuestProbe for Probe {
    async fn ready(&self, _namespace: &str, vm_name: &str) -> Result<(), Failure> {
        self.probed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(vm_name.to_owned());
        if self.fail {
            return Err("the guest did not answer".into());
        }
        Ok(())
    }
}

/// A provider over a fake cluster with its custody directory.
pub struct Harness {
    pub cluster: Arc<FakeCluster>,
    pub dir: TempDir,
    pub backend: Arc<Backend>,
    pub probe: Arc<Probe>,
    pub provider: KubeVirtProvider,
}

impl Harness {
    pub fn new() -> Boxed<Self> {
        Self::with(Arc::new(Backend::new()), Arc::new(Probe::default()))
    }

    pub fn with(backend: Arc<Backend>, probe: Arc<Probe>) -> Boxed<Self> {
        let cluster = Arc::new(FakeCluster::new());
        Self::over(cluster, TempDir::new()?, backend, probe)
    }

    pub fn over(cluster: Arc<FakeCluster>, dir: TempDir, backend: Arc<Backend>, probe: Arc<Probe>) -> Boxed<Self> {
        let provider = build(&cluster, dir.path(), backend.clone(), probe.clone())?;
        Ok(Self {
            cluster,
            dir,
            backend,
            probe,
            provider,
        })
    }

    /// A second provider over the same cluster and custody directory, as a restarted worker would be. The first
    /// must be dropped before so that its directory lock is released.
    pub fn restart(self) -> Boxed<Self> {
        let Self {
            cluster,
            dir,
            backend,
            probe,
            provider,
        } = self;
        drop(provider);
        Self::over(cluster, dir, backend, probe)
    }
}

pub fn build(cluster: &FakeCluster, state: &Path, backend: Arc<Backend>, probe: Arc<Probe>) -> Boxed<KubeVirtProvider> {
    let provider = KubeVirtProvider::new(config(cluster)?, cluster.client(), state, backend)?;
    Ok(provider.with_guest_probe(probe))
}

pub fn config(cluster: &FakeCluster) -> Boxed<KubeVirtConfig> {
    let mut config = support::config(&cluster.namespace())?;
    config.volume = Some(support::volume(Path::new("/nonexistent")));
    config.warm_pool.max_total_warm = 0;
    Ok(config)
}

pub fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

/// Parts of a spec with `image`, which tests change before building the spec.
pub fn parts(image: SandboxImage) -> Boxed<SandboxSpecParts> {
    Ok(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(60).ok_or("cpu")?,
        memory_mib: NonZeroU32::new(256).ok_or("memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: SecretBindings::default(),
        platform: PlatformOptions::default(),
        idle_timeout_seconds: None,
        no_ttl: LeaseTtl::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: RetentionToggles::default(),
    })
}

pub fn request_with(parts: SandboxSpecParts) -> Boxed<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("tenant-1")?,
        provider_pref: vec![ProviderKind::KubeVirt],
        spec: SandboxSpec::new(parts)?,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("key-1")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: LeasePrewarm::Regular,
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

pub fn registry_image() -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified("registry.example/sandbox/base:1"),
        pull_secret: None,
    }
}

pub fn snapshot_image(artifact: OciArtifactRef) -> SandboxImage {
    SandboxImage::Snapshot {
        reference: artifact,
        parent: None,
    }
}

/// A cold start request from a registry image.
pub fn registry_request() -> Boxed<LeaseRequest> {
    request_with(parts(registry_image())?)
}

/// A request that restores `artifact`.
pub fn snapshot_request(artifact: OciArtifactRef) -> Boxed<LeaseRequest> {
    request_with(parts(snapshot_image(artifact))?)
}

pub fn ok() -> TestResult {
    Ok(())
}

/// Passes when `error` is a backend failure of the provider that `expected` accepts.
pub fn refused(error: ProviderError, expected: impl FnOnce(&KubeVirtProviderError) -> bool) -> TestResult {
    let ProviderError::Backend(inner) = error else {
        return Err(format!("not a backend failure: {error:?}").into());
    };
    let kind = inner
        .downcast::<KubeVirtProviderError>()
        .map_err(|other| format!("unexpected {other:?}"))?;
    if expected(&kind) {
        Ok(())
    } else {
        Err(format!("unexpected provider error {kind:?}").into())
    }
}

/// Passes when `error` is one `expected` accepts.
pub fn rejected(error: KubeVirtProviderError, expected: impl FnOnce(&KubeVirtProviderError) -> bool) -> TestResult {
    if expected(&error) {
        Ok(())
    } else {
        Err(format!("unexpected provider error {error:?}").into())
    }
}
