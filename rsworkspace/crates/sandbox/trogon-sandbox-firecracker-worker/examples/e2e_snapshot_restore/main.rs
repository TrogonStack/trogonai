//! Proves a running microVM can be checkpointed to an OCI registry and restored as new leases:
//! the restored guests keep the captured disk and memory, take their own identity, draw
//! independent randomness, and see a wall clock that advanced with the host, while a warm pool
//! lease of the same image never sees the captured tenant's state.
//!
//! Needs a Linux host with /dev/kvm, a `firecracker` binary, a kernel and rootfs the guest agent
//! is installed in, and a plain HTTP OCI registry. Configured through the environment:
//!
//! - `E2E_FIRECRACKER_BINARY`, `E2E_KERNEL` and `E2E_ROOTFS` (required)
//! - `E2E_STATE_DIR` (required, an empty directory dedicated to this run)
//! - `E2E_REGISTRY` (optional, defaults to `127.0.0.1:5000`)
//! - `E2E_JAILER` (optional, a `jailer` binary every VMM runs under)

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::num::NonZeroU32;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
use anyhow::{Context, ensure};
#[cfg(target_os = "linux")]
use trogon_sandbox_contract::{
    ArtifactType, CommandEnv, CommandExecution, CommandOutput, Digest, LaunchCancellation, LeaseId, LeasePersistence,
    LeasePrewarm, LeaseRequest, LeaseRequestKey, OciArtifactRef, OciImageRef, OutputRetention, ProviderKind,
    RedactValues, RunCommandRequest, RunCommandResponse, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    StreamedOutput, TenantId, WorkspaceSource,
};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::FirecrackerProvider;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::agent::DiscardChunks;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::config::{
    BaseRootfs, FirecrackerBinary, FirecrackerConfig, KernelImage, MemoryMib, StateDir,
};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::jailer::{JailerConfig, JailerLauncher};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::process::{DirectLauncher, VmLauncher};
#[cfg(target_os = "linux")]
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};
#[cfg(target_os = "linux")]
use trogon_sandbox_protocol::Provider;
#[cfg(target_os = "linux")]
use trogon_std::env::{ReadEnv, SystemEnv};

#[cfg(target_os = "linux")]
mod constants;
#[cfg(target_os = "linux")]
use constants::{CLOCK_DELAY_SECONDS, CLOCK_TOLERANCE_SECONDS, IMAGE, JAILED_ID};

#[cfg(target_os = "linux")]
fn required(name: &str) -> anyhow::Result<String> {
    SystemEnv.var(name).with_context(|| format!("{name} must be set"))
}

#[cfg(target_os = "linux")]
fn launcher(config: &FirecrackerConfig) -> anyhow::Result<Arc<dyn VmLauncher>> {
    Ok(match SystemEnv.var("E2E_JAILER") {
        Ok(binary) => Arc::new(JailerLauncher::new(
            JailerConfig {
                binary: binary.into(),
                uid_base: NonZeroU32::new(JAILED_ID).context("zero jailer uid")?,
                gid_base: NonZeroU32::new(JAILED_ID).context("zero jailer gid")?,
                cgroup_parent: "sandbox-fc-snapshot".into(),
                memory_overhead: MemoryMib::new(128)?,
            },
            config,
        )?),
        Err(_) => Arc::new(DirectLauncher::new(config.binary.clone())),
    })
}

#[cfg(target_os = "linux")]
fn request(image: SandboxImage, env: BTreeMap<String, String>) -> anyhow::Result<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("snapshot-e2e")?,
        provider_pref: vec![ProviderKind::Firecracker],
        spec: SandboxSpec::new(SandboxSpecParts {
            image,
            workspace: WorkspaceSource::empty(),
            env,
            cpu_seconds: NonZeroU32::new(120).context("zero cpu seconds")?,
            memory_mib: NonZeroU32::new(256).context("zero memory")?,
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
        })?,
        ttl_seconds: 120,
        idempotency_key: LeaseRequestKey::new("snapshot-e2e")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::from(false),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::from(false),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

#[cfg(target_os = "linux")]
async fn exec(provider: &FirecrackerProvider, lease: &LeaseId, script: &str) -> anyhow::Result<CommandOutput> {
    let command = RunCommandRequest {
        lease_id: lease.clone(),
        command: vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()],
        cwd: Some("/".to_string()),
        env: CommandEnv::default(),
        timeout_seconds: 10,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: OutputRetention::default(),
        max_output_bytes: None,
        redact_values: RedactValues::default(),
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::NotRequired,
        execution: CommandExecution::Attached,
    };
    let response = provider
        .agent_for(lease)
        .await
        .context("lease lost its agent")?
        .exec(&command, &DiscardChunks)
        .await?;
    let RunCommandResponse::Completed(output) = response else {
        anyhow::bail!("unexpected exec response: {response:?}");
    };
    ensure!(output.exit_code == 0, "guest command failed: {output:?}");
    Ok(output)
}

#[cfg(target_os = "linux")]
async fn random_draw(provider: &FirecrackerProvider, lease: &LeaseId) -> anyhow::Result<Vec<u8>> {
    let output = exec(provider, lease, "od -An -tx1 -N32 /dev/urandom | tr -d ' \\n'").await?;
    ensure!(
        output.stdout.len() == 64 && output.stdout.iter().all(u8::is_ascii_hexdigit),
        "guest random draw must contain exactly 32 hexadecimal bytes"
    );
    Ok(output.stdout)
}

#[cfg(target_os = "linux")]
#[derive(serde::Serialize)]
struct ClockObservation {
    guest_seconds: u64,
    host_before_seconds: u64,
    host_after_seconds: u64,
    clocksource: &'static str,
}

#[cfg(target_os = "linux")]
fn host_seconds() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(target_os = "linux")]
async fn observe_clock(provider: &FirecrackerProvider, lease: &LeaseId) -> anyhow::Result<ClockObservation> {
    let host_before_seconds = host_seconds()?;
    let output = exec(
        provider,
        lease,
        "set -e; cat /sys/devices/system/clocksource/clocksource0/current_clocksource; date +%s",
    )
    .await?;
    let host_after_seconds = host_seconds()?;
    let text = std::str::from_utf8(&output.stdout)?;
    let mut lines = text.lines();
    ensure!(lines.next() == Some("kvm-clock"), "guest must report kvm-clock");
    let guest_seconds = lines.next().context("guest date output is missing")?.parse::<u64>()?;
    ensure!(lines.next().is_none(), "guest clock output has unexpected lines");
    ensure!(
        host_after_seconds >= host_before_seconds,
        "host wall clock moved backwards during observation"
    );
    ensure!(
        guest_seconds.saturating_add(CLOCK_TOLERANCE_SECONDS) >= host_before_seconds
            && guest_seconds <= host_after_seconds.saturating_add(CLOCK_TOLERANCE_SECONDS),
        "guest wall clock differs from host: guest={guest_seconds}, host={host_before_seconds}..={host_after_seconds}"
    );
    Ok(ClockObservation {
        guest_seconds,
        host_before_seconds,
        host_after_seconds,
        clocksource: "kvm-clock",
    })
}

#[cfg(target_os = "linux")]
fn verify_clock_advance(captured: &ClockObservation, restored: &ClockObservation) -> anyhow::Result<()> {
    let minimum_elapsed = restored
        .host_before_seconds
        .checked_sub(captured.host_after_seconds)
        .context("host wall clock moved backwards since capture")?;
    let maximum_elapsed = restored
        .host_after_seconds
        .checked_sub(captured.host_before_seconds)
        .context("host wall clock moved backwards since capture")?;
    let guest_elapsed = restored
        .guest_seconds
        .checked_sub(captured.guest_seconds)
        .context("restored guest wall clock moved backwards")?;
    ensure!(
        minimum_elapsed >= CLOCK_DELAY_SECONDS,
        "clock probe needs at least three seconds since capture"
    );
    ensure!(
        guest_elapsed >= CLOCK_DELAY_SECONDS
            && guest_elapsed.saturating_add(CLOCK_TOLERANCE_SECONDS) >= minimum_elapsed
            && guest_elapsed <= maximum_elapsed.saturating_add(CLOCK_TOLERANCE_SECONDS),
        "restored clock did not advance with elapsed host time: guest={guest_elapsed}s, host={minimum_elapsed}..={maximum_elapsed}s"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
/// What the run measured and proved, printed as one JSON line.
#[derive(serde::Serialize)]
struct Summary {
    cold_with_pool_seed_ms: u128,
    warm_lease_ms: u128,
    oci_restore_ms: u128,
    snapshot_bytes: u64,
    snapshot_digest: String,
    disk_and_ram_preserved: bool,
    warm_tenant_isolation: bool,
    restored_identity: bool,
    independent_clone_random_draws_distinct: bool,
    randomness_scope: &'static str,
    clock_delay_after_checkpoint_seconds: u64,
    clock_tolerance_seconds: u64,
    captured_clock: ClockObservation,
    warm_clock: ClockObservation,
    restored_clock: ClockObservation,
    other_clock: ClockObservation,
}

#[cfg(target_os = "linux")]
async fn exercise(
    provider: &FirecrackerProvider,
    registry: &str,
    published: &mut Vec<OciArtifactRef>,
) -> anyhow::Result<()> {
    let pid = std::process::id();
    let source = LeaseId::new(format!("snapshot-source-{pid}"));
    let warm = LeaseId::new(format!("snapshot-warm-{pid}"));
    let restored = LeaseId::new(format!("snapshot-restored-{pid}"));
    let other = LeaseId::new(format!("snapshot-other-{pid}"));
    let image = || SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(IMAGE),
        pull_secret: None,
    };

    let source_request = request(
        image(),
        BTreeMap::from([("SNAPSHOT_SECRET".to_string(), "source-only".to_string())]),
    )?;
    let started = Instant::now();
    provider.lease(&source_request, &source).await?;
    let cold_ms = started.elapsed().as_millis();
    let captured_clock = observe_clock(provider, &source).await?;
    exec(
        provider,
        &source,
        "printf disk-proof > /snapshot-disk; printf ram-proof > /run/snapshot-ram",
    )
    .await?;
    let target = OciArtifactRef {
        registry: registry.to_string(),
        repository: format!("firecracker-e2e/snapshot-{pid}"),
        digest: Digest::new(format!("sha256:{}", "0".repeat(64)))?,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    };
    let receipt = provider.checkpoint_running(&source, &target).await?;
    let snapshot = OciArtifactRef {
        digest: receipt.digest.clone(),
        ..target
    };
    published.push(snapshot.clone());
    tokio::time::sleep(Duration::from_secs(CLOCK_DELAY_SECONDS)).await;
    exec(
        provider,
        &source,
        "test \"$(cat /snapshot-disk)\" = disk-proof && test \"$(cat /run/snapshot-ram)\" = ram-proof",
    )
    .await?;
    provider.cancel(&source).await?;

    let started = Instant::now();
    provider.lease(&request(image(), BTreeMap::new())?, &warm).await?;
    let warm_ms = started.elapsed().as_millis();
    let warm_clock = observe_clock(provider, &warm).await?;
    verify_clock_advance(&captured_clock, &warm_clock)?;
    exec(
        provider,
        &warm,
        &format!(
            "test ! -e /snapshot-disk && test ! -e /run/snapshot-ram && test -z \"$SNAPSHOT_SECRET\" && test \"$(cat /run/ate/actor-id)\" = {}",
            warm.as_str()
        ),
    )
    .await?;
    provider.cancel(&warm).await?;

    let restore_request = request(
        SandboxImage::Snapshot {
            reference: snapshot,
            parent: None,
        },
        BTreeMap::new(),
    )?;
    let started = Instant::now();
    provider.lease(&restore_request, &restored).await?;
    let restore_ms = started.elapsed().as_millis();
    let first_random = random_draw(provider, &restored).await?;
    let restored_clock = observe_clock(provider, &restored).await?;
    verify_clock_advance(&captured_clock, &restored_clock)?;
    exec(
        provider,
        &restored,
        &format!(
            "test \"$(cat /snapshot-disk)\" = disk-proof && test \"$(cat /run/snapshot-ram)\" = ram-proof && test -z \"$SNAPSHOT_SECRET\" && test \"$(cat /run/ate/actor-id)\" = {}",
            restored.as_str()
        ),
    )
    .await?;
    exec(
        provider,
        &restored,
        "printf clone-only > /snapshot-first-only; printf clone-only > /run/snapshot-first-only",
    )
    .await?;
    provider.cancel(&restored).await?;

    provider.lease(&restore_request, &other).await?;
    let other_random = random_draw(provider, &other).await?;
    let other_clock = observe_clock(provider, &other).await?;
    verify_clock_advance(&captured_clock, &other_clock)?;
    ensure!(
        first_random != other_random,
        "independent restores repeated their first post-bind random draw"
    );
    exec(
        provider,
        &other,
        &format!(
            "test ! -e /snapshot-first-only && test ! -e /run/snapshot-first-only && test \"$(cat /run/ate/actor-id)\" = {}",
            other.as_str()
        ),
    )
    .await?;
    provider.cancel(&other).await?;
    ensure!(
        provider.current_lease_count().await == 0,
        "leases survived cancellation"
    );

    let summary = Summary {
        cold_with_pool_seed_ms: cold_ms,
        warm_lease_ms: warm_ms,
        oci_restore_ms: restore_ms,
        snapshot_bytes: receipt.bytes,
        snapshot_digest: receipt.digest.as_str().to_string(),
        disk_and_ram_preserved: true,
        warm_tenant_isolation: true,
        restored_identity: true,
        independent_clone_random_draws_distinct: true,
        randomness_scope: "guest_kernel_rng_only",
        clock_delay_after_checkpoint_seconds: CLOCK_DELAY_SECONDS,
        clock_tolerance_seconds: CLOCK_TOLERANCE_SECONDS,
        captured_clock,
        warm_clock,
        restored_clock,
        other_clock,
    };
    writeln!(std::io::stdout(), "{}", serde_json::to_string(&summary)?)?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn run() -> anyhow::Result<()> {
    let state = required("E2E_STATE_DIR")?;
    let registry = SystemEnv
        .var("E2E_REGISTRY")
        .unwrap_or_else(|_| "127.0.0.1:5000".to_string());
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new(required("E2E_FIRECRACKER_BINARY")?),
        KernelImage::new(required("E2E_KERNEL")?),
        BaseRootfs::new(required("E2E_ROOTFS")?),
        StateDir::new(&state),
    );
    config.default_image = OciImageRef::parse_unqualified(IMAGE);
    config.warm_pool.max_snapshots = 1;
    let oci = Arc::new(OciClient::new(OciClientConfig {
        insecure_registries: vec![RegistryHost::new(registry.clone())?],
        ..Default::default()
    }));
    let provider = FirecrackerProvider::new(config.clone(), launcher(&config)?).with_oci_client(Arc::clone(&oci));
    provider.clean_stale().await;

    let mut published = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(150), exercise(&provider, &registry, &mut published)).await;
    provider.shutdown().await;
    provider.clear_warm_pool().await;
    for reference in published {
        oci.delete_artifact(&reference).await?;
    }
    result.context("snapshot example exceeded 150 seconds")??;
    ensure!(
        std::fs::read_dir(&state)?.next().is_none(),
        "state directories survived cleanup"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        b"e2e_snapshot_restore needs Linux with /dev/kvm\n",
    );
}
