//! Snapshot export and restore, the warm pool and live checkpoints against a real KubeVirt cluster with CDI.
//!
//! Skipped unless `SANDBOX_KUBEVIRT_LIVE_TEST=1`. The worker's own variables configure the run
//! (`SANDBOX_K8S_NAMESPACE` and `SANDBOX_KUBEVIRT_IMAGE` are required, the snapshot registry, S3 volume store and
//! storage class are optional) and the kubeconfig selects the cluster. Run with `--ignored`.

use std::error::Error;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use trogon_sandbox_contract::{
    ArtifactType, BootMode, Digest, LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, LeaseTtl,
    OciArtifactRef, OciImageRef, PlatformOptions, PrivilegedAccess, ProviderKind, RetentionToggles, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, SecretBindings, TenantId, WorkspaceSource,
};
use trogon_sandbox_kubevirt_worker::{
    CHECKPOINT_PLACEHOLDER_DIGEST, KubeVirtConfig, KubeVirtOps, KubeVirtProvider, S3RestoreStores, SnapshotService,
    WarmPoolConfig, WorkerArgs,
};
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_protocol::Provider;
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult = Result<(), Box<dyn Error>>;

const GATE: &str = "SANDBOX_KUBEVIRT_LIVE_TEST";
const SNAPSHOT_REPOSITORY: &str = "snapshots/live/kubevirt";
const WARM_POPULATION_TIMEOUT: Duration = Duration::from_secs(900);
const WARM_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// A run against a real cluster: the provider, its configuration and a way to look at the cluster.
struct Live {
    provider: KubeVirtProvider,
    ops: KubeVirtOps,
    config: KubeVirtConfig,
    _state: tempfile::TempDir,
}

fn enabled() -> bool {
    let on = SystemEnv.var(GATE).is_ok_and(|value| value == "1");
    if !on {
        eprintln!("skipped: set {GATE}=1 to run against a real cluster");
    }
    on
}

fn suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

async fn connect(warm_pool: Option<WarmPoolConfig>) -> Result<Live, Box<dyn Error>> {
    let args = WorkerArgs::try_parse_from(["live"])?;
    let mut config = args.kube_virt_config()?;
    if let Some(warm_pool) = warm_pool {
        config.warm_pool = warm_pool;
    }
    let client = kube::Client::try_default().await?;
    let snapshots = Arc::new(SnapshotService::new(
        Arc::new(OciClient::new(config.oci.clone())),
        Arc::new(S3RestoreStores::new(SystemEnv)),
    ));
    let state = tempfile::TempDir::new()?;
    let provider = KubeVirtProvider::new(config.clone(), client.clone(), Path::new(state.path()), snapshots)?;
    Ok(Live {
        provider,
        ops: KubeVirtOps::new(client, config.clone()),
        config,
        _state: state,
    })
}

fn request(image: SandboxImage, key: &str) -> Result<LeaseRequest, Box<dyn Error>> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: NonZeroU32::new(60).ok_or("cpu")?,
        memory_mib: NonZeroU32::new(512).ok_or("memory")?,
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
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("live")?,
        provider_pref: vec![ProviderKind::KubeVirt],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new(key)?,
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

fn base_image(live: &Live) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(&live.config.default_image),
        pull_secret: None,
    }
}

fn target(live: &Live, digest: &str) -> Result<OciArtifactRef, Box<dyn Error>> {
    Ok(OciArtifactRef {
        registry: live.config.snapshot_registry.clone(),
        repository: SNAPSHOT_REPOSITORY.to_owned(),
        digest: Digest::new(digest)?,
        artifact_type: ArtifactType::KubevirtDiskV1,
    })
}

fn restore_image(live: &Live, digest: Digest) -> Result<SandboxImage, Box<dyn Error>> {
    let mut reference = target(live, CHECKPOINT_PLACEHOLDER_DIGEST)?;
    reference.digest = digest;
    Ok(SandboxImage::Snapshot {
        reference,
        parent: None,
    })
}

#[tokio::test]
#[ignore = "requires a KubeVirt cluster with CDI and an OCI registry; set SANDBOX_KUBEVIRT_LIVE_TEST=1 and use --ignored"]
async fn a_snapshot_exported_from_a_lease_restores_into_a_new_one() -> TestResult {
    if !enabled() {
        return Ok(());
    }
    let live = connect(None).await?;
    let lease_id = LeaseId::new(format!("live-snapshot-{}", suffix()));
    let first = live
        .provider
        .lease(&request(base_image(&live), "live-snapshot-1")?, &lease_id)
        .await?;
    let receipt = live
        .provider
        .snapshot(&lease_id, &target(&live, CHECKPOINT_PLACEHOLDER_DIGEST)?)
        .await?;
    live.provider.cancel(&lease_id).await?;

    if live.config.volume.is_none() {
        eprintln!("restore leg skipped: set SANDBOX_VOLUME_S3_BUCKET for the full round trip");
        return Ok(());
    }
    let restore_id = LeaseId::new(format!("live-restore-{}", suffix()));
    let restored = live
        .provider
        .lease(
            &request(restore_image(&live, receipt.digest)?, "live-snapshot-2")?,
            &restore_id,
        )
        .await?;
    assert_ne!(restored.sandbox_id, first.sandbox_id);
    live.provider.cancel(&restore_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a KubeVirt cluster with CDI, an OCI registry and an S3 store; set SANDBOX_KUBEVIRT_LIVE_TEST=1 and use --ignored"]
async fn a_warm_vm_serves_a_restore_faster_than_a_cold_one() -> TestResult {
    if !enabled() {
        return Ok(());
    }
    let warm_pool = WarmPoolConfig {
        enabled: true,
        max_pool_size_per_digest: 1,
        max_total_warm: 4,
        max_idle_seconds: 1800,
        warm_memory_mib: 512,
    };
    let live = connect(Some(warm_pool)).await?;
    if live.config.volume.is_none() {
        eprintln!("skipped: set SANDBOX_VOLUME_S3_BUCKET for the warm pool");
        return Ok(());
    }

    let seed = LeaseId::new(format!("live-warm-seed-{}", suffix()));
    live.provider
        .lease(&request(base_image(&live), "live-warm-seed")?, &seed)
        .await?;
    let receipt = live
        .provider
        .snapshot(&seed, &target(&live, CHECKPOINT_PLACEHOLDER_DIGEST)?)
        .await?;
    live.provider.cancel(&seed).await?;
    let digest = receipt.digest;

    let cold_id = LeaseId::new(format!("live-warm-cold-{}", suffix()));
    let started = Instant::now();
    let cold = live
        .provider
        .lease(
            &request(restore_image(&live, digest.clone())?, "live-warm-cold")?,
            &cold_id,
        )
        .await?;
    let cold_elapsed = started.elapsed();
    live.provider.cancel(&cold_id).await?;

    let deadline = Instant::now() + WARM_POPULATION_TIMEOUT;
    loop {
        let report = live.provider.warm_pool().population_report().await;
        if report.get(&digest).copied().unwrap_or(0) >= 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the warm pool did not fill after a cold restore"
        );
        tokio::time::sleep(WARM_POLL_INTERVAL).await;
    }

    let warm_id = LeaseId::new(format!("live-warm-hot-{}", suffix()));
    let started = Instant::now();
    let warm = live
        .provider
        .lease(&request(restore_image(&live, digest)?, "live-warm-hot")?, &warm_id)
        .await?;
    let warm_elapsed = started.elapsed();
    assert_ne!(warm.sandbox_id, cold.sandbox_id);
    assert!(
        warm_elapsed < cold_elapsed,
        "a warm lease ({warm_elapsed:?}) should beat a cold restore ({cold_elapsed:?})"
    );
    live.provider.cancel(&warm_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a KubeVirt cluster with CDI and an OCI registry; set SANDBOX_KUBEVIRT_LIVE_TEST=1 and use --ignored"]
async fn a_live_checkpoint_keeps_the_vm_running() -> TestResult {
    if !enabled() {
        return Ok(());
    }
    let live = connect(None).await?;
    let lease_id = LeaseId::new(format!("live-checkpoint-{}", suffix()));
    let lease = live
        .provider
        .lease(&request(base_image(&live), "live-checkpoint")?, &lease_id)
        .await?;
    let vm = lease.sandbox_id.as_str();
    assert!(
        live.ops.vmi_is_running(vm).await?,
        "the VMI must run before the checkpoint"
    );

    let receipt = live
        .provider
        .checkpoint_running(&lease_id, &target(&live, CHECKPOINT_PLACEHOLDER_DIGEST)?)
        .await?;
    assert!(receipt.bytes > 0, "the checkpoint exported no disk bytes");
    assert!(
        live.ops.vmi_is_running(vm).await?,
        "the VMI must keep running after the checkpoint"
    );
    live.provider.cancel(&lease_id).await?;
    Ok(())
}
