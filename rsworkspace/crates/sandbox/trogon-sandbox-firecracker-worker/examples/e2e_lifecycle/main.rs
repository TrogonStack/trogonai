//! Proves a real microVM's resources are reclaimed on every way a lease can end badly: the
//! VMM dies under a live lease, the guest agent never answers, and the worker itself exits
//! and leaves an orphaned VMM for the next worker on the host to recover.
//!
//! Needs a Linux host with /dev/kvm, a `firecracker` binary, and a kernel and rootfs the
//! guest agent is installed in. Configured through the environment:
//!
//! - `E2E_FIRECRACKER_BINARY`, `E2E_KERNEL` and `E2E_ROOTFS` (required)
//! - `E2E_STATE_DIR` (required, an empty directory dedicated to this run)
//! - `E2E_JAILER` (optional, a `jailer` binary; when set every VMM runs jailed and the jail,
//!   its seccomp filter and its cgroup limits are verified too)

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::num::NonZeroU32;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use anyhow::{Context, ensure};
#[cfg(target_os = "linux")]
use trogon_sandbox_contract::{
    CommandEnv, CommandExecution, LaunchCancellation, LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest,
    LeaseRequestKey, OciImageRef, OutputRetention, ProviderKind, RedactValues, RunCommandRequest, RunCommandResponse,
    SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, StreamedOutput, TenantId, WorkspaceSource,
};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::FirecrackerProvider;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::agent::{AgentError, DiscardChunks};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::config::{
    BaseRootfs, FirecrackerBinary, FirecrackerConfig, KernelImage, MemoryMib, StateDir, VsockPort,
};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::error::ProviderError;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::jailer::{JailerConfig, JailerLauncher};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::process::{DirectLauncher, VmLauncher};
#[cfg(target_os = "linux")]
use trogon_sandbox_protocol::Provider;
#[cfg(target_os = "linux")]
use trogon_std::env::{ReadEnv, SystemEnv};

#[cfg(target_os = "linux")]
mod constants;
#[cfg(target_os = "linux")]
use constants::{CGROUP_PARENT, IMAGE, JAILED_ID};

#[cfg(target_os = "linux")]
fn required(name: &str) -> anyhow::Result<String> {
    SystemEnv.var(name).with_context(|| format!("{name} must be set"))
}

#[cfg(target_os = "linux")]
fn jailed() -> bool {
    SystemEnv.var("E2E_JAILER").is_ok()
}

#[cfg(target_os = "linux")]
fn launcher(config: &FirecrackerConfig) -> anyhow::Result<Arc<dyn VmLauncher>> {
    Ok(match SystemEnv.var("E2E_JAILER") {
        Ok(binary) => Arc::new(JailerLauncher::new(
            JailerConfig {
                binary: binary.into(),
                uid_base: NonZeroU32::new(JAILED_ID).context("zero jailer uid")?,
                gid_base: NonZeroU32::new(JAILED_ID).context("zero jailer gid")?,
                cgroup_parent: CGROUP_PARENT.into(),
                memory_overhead: MemoryMib::new(128)?,
            },
            config,
        )?),
        Err(_) => Arc::new(DirectLauncher::new(config.binary.clone())),
    })
}

#[cfg(target_os = "linux")]
fn request() -> anyhow::Result<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("lifecycle-e2e")?,
        provider_pref: vec![ProviderKind::Firecracker],
        spec: SandboxSpec::new(SandboxSpecParts {
            image: SandboxImage::Registry {
                reference: OciImageRef::parse_unqualified(IMAGE),
                pull_secret: None,
            },
            workspace: WorkspaceSource::empty(),
            env: BTreeMap::new(),
            cpu_seconds: NonZeroU32::new(60).context("zero cpu seconds")?,
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
        ttl_seconds: 60,
        idempotency_key: LeaseRequestKey::new("lifecycle")?,
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
async fn pid(config: &FirecrackerConfig, lease: &LeaseId) -> anyhow::Result<u32> {
    Ok(
        tokio::fs::read_to_string(config.state_dir.as_path().join(lease.as_str()).join("firecracker.pid"))
            .await?
            .trim()
            .parse()?,
    )
}

#[cfg(target_os = "linux")]
async fn verify_jail(config: &FirecrackerConfig, lease: &LeaseId, pid: u32) -> anyhow::Result<()> {
    if !jailed() {
        return Ok(());
    }
    let expected = launcher(config)?.vm_dir(&config.state_dir, lease)?;
    let actual_root = tokio::fs::metadata(format!("/proc/{pid}/root")).await?;
    let expected_root = tokio::fs::metadata(expected.runtime_dir()).await?;
    ensure!(
        (actual_root.dev(), actual_root.ino()) == (expected_root.dev(), expected_root.ino()),
        "VMM root escaped its jail"
    );
    let id = JAILED_ID.to_string();
    let uid_line = ["Uid:", id.as_str(), id.as_str(), id.as_str(), id.as_str()];
    let mut tasks = tokio::fs::read_dir(format!("/proc/{pid}/task")).await?;
    while let Some(task) = tasks.next_entry().await? {
        let status = tokio::fs::read_to_string(task.path().join("status")).await?;
        ensure!(
            status
                .lines()
                .any(|line| line.split_whitespace().collect::<Vec<_>>() == uid_line),
            "VMM thread retained a host UID"
        );
        ensure!(
            status
                .lines()
                .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["Seccomp:", "2"]),
            "VMM thread has no seccomp filter"
        );
    }
    let group = format!("{CGROUP_PARENT}/{}/vm", lease.as_str());
    ensure!(
        tokio::fs::read_to_string(format!("/proc/{pid}/cgroup")).await?.trim() == format!("0::/{group}"),
        "VMM has no dedicated cgroup"
    );
    for (file, expected) in [
        ("memory.max", "402653184"),
        ("cpu.max", "200000 100000"),
        ("pids.max", "34"),
    ] {
        ensure!(
            tokio::fs::read_to_string(format!("/sys/fs/cgroup/{group}/{file}"))
                .await?
                .trim()
                == expected,
            "incorrect {file}"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn whoami(lease: &LeaseId) -> RunCommandRequest {
    RunCommandRequest {
        lease_id: lease.clone(),
        command: vec!["id".to_string(), "-u".to_string()],
        cwd: Some("/".to_string()),
        env: CommandEnv::default(),
        timeout_seconds: 5,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: OutputRetention::default(),
        max_output_bytes: None,
        redact_values: RedactValues::default(),
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::NotRequired,
        execution: CommandExecution::Attached,
    }
}

#[cfg(target_os = "linux")]
/// Whether `/proc/<pid>/stat` describes a process that is gone or only waiting to be reaped.
fn exited(stat: &str) -> bool {
    stat.is_empty()
        || stat
            .rsplit_once(") ")
            .is_some_and(|(_, tail)| tail.starts_with('Z') || tail.starts_with('X'))
}

#[cfg(target_os = "linux")]
/// Leases one microVM, prints its VMM pid, and exits without running any cleanup, so the
/// VMM outlives the worker that started it.
async fn leave_an_orphan(config: FirecrackerConfig) -> anyhow::Result<()> {
    let provider = FirecrackerProvider::new(config.clone(), launcher(&config)?);
    let lease = LeaseId::new("orphan");
    provider.lease(&request()?, &lease).await?;
    writeln!(std::io::stdout(), "{}", pid(&config, &lease).await?)?;
    std::io::stdout().flush()?;
    std::process::exit(0);
}

#[cfg(target_os = "linux")]
/// What the run proved, printed as one JSON line.
#[derive(serde::Serialize)]
struct Summary {
    vmm_exit_reclaimed: bool,
    unresponsive_agent_reclaimed: bool,
    worker_crash_reclaimed: bool,
    jailer_verified: bool,
}

#[cfg(target_os = "linux")]
async fn exercise(config: &FirecrackerConfig) -> anyhow::Result<()> {
    let provider = FirecrackerProvider::new(config.clone(), launcher(config)?);
    ensure!(
        provider.recover_stale().await? == 0,
        "use an empty dedicated E2E_STATE_DIR"
    );
    let lease = LeaseId::new("killed");
    provider.lease(&request()?, &lease).await?;
    let process = pid(config, &lease).await?;
    let outcome = provider
        .agent_for(&lease)
        .await
        .context("lease lost agent")?
        .exec(&whoami(&lease), &DiscardChunks)
        .await?;
    ensure!(
        matches!(outcome, RunCommandResponse::Completed(ref output) if output.exit_code == 0 && output.stdout == b"0\n"),
        "guest has no root exec"
    );
    verify_jail(config, &lease, process).await?;
    ensure!(
        tokio::process::Command::new("kill")
            .args(["-TERM", &process.to_string()])
            .status()
            .await?
            .success(),
        "could not inject VMM exit"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.current_lease_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("dead VMM retained its capacity")?;
    ensure!(
        !config.state_dir.as_path().join("killed").exists(),
        "dead VMM retained files"
    );
    drop(provider);

    let mut unresponsive = config.clone();
    unresponsive.agent_port = VsockPort::new(65535)?;
    unresponsive.boot_timeout = Duration::from_secs(3);
    let unresponsive = FirecrackerProvider::new(unresponsive.clone(), launcher(&unresponsive)?);
    match unresponsive
        .lease_inner(&request()?, &LeaseId::new("unresponsive"))
        .await
    {
        Err(ProviderError::Agent(AgentError::NotReady { .. })) => {}
        other => anyhow::bail!("wrong readiness failure: {other:?}"),
    }
    ensure!(
        !config.state_dir.as_path().join("unresponsive").exists(),
        "unresponsive guest retained files"
    );
    drop(unresponsive);

    let child = tokio::process::Command::new(std::env::current_exe()?)
        .env("E2E_ORPHAN_ONLY", "1")
        .kill_on_drop(true)
        .output()
        .await?;
    ensure!(
        child.status.success(),
        "orphan worker failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let orphan_pid: u32 = String::from_utf8(child.stdout)?.trim().parse()?;
    ensure!(
        tokio::fs::try_exists(format!("/proc/{orphan_pid}")).await?,
        "worker exit did not leave a live orphan"
    );
    let restarted = FirecrackerProvider::new(config.clone(), launcher(config)?);
    ensure!(
        restarted.recover_stale().await? == 1,
        "restart did not clean its live orphan"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while tokio::fs::try_exists(format!("/proc/{orphan_pid}")).await? {
            let stat = tokio::fs::read_to_string(format!("/proc/{orphan_pid}/stat"))
                .await
                .unwrap_or_default();
            ensure!(exited(&stat), "orphan {orphan_pid} is still running: {stat}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        anyhow::Ok(())
    })
    .await
    .context("init did not reap the terminated orphan within 5 seconds")??;
    let fresh = LeaseId::new("after-restart");
    restarted.lease(&request()?, &fresh).await?;
    verify_jail(config, &fresh, pid(config, &fresh).await?).await?;
    restarted.cancel(&fresh).await?;
    ensure!(
        std::fs::read_dir(config.state_dir.as_path())?.next().is_none(),
        "lease directories survived cleanup"
    );
    let summary = Summary {
        vmm_exit_reclaimed: true,
        unresponsive_agent_reclaimed: true,
        worker_crash_reclaimed: true,
        jailer_verified: jailed(),
    };
    writeln!(std::io::stdout(), "{}", serde_json::to_string(&summary)?)?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn run() -> anyhow::Result<()> {
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new(required("E2E_FIRECRACKER_BINARY")?),
        KernelImage::new(required("E2E_KERNEL")?),
        BaseRootfs::new(required("E2E_ROOTFS")?),
        StateDir::new(required("E2E_STATE_DIR")?),
    );
    config.default_image = OciImageRef::parse_unqualified(IMAGE);
    config.boot_timeout = Duration::from_secs(10);
    if SystemEnv.var("E2E_ORPHAN_ONLY").is_ok() {
        return leave_an_orphan(config).await;
    }

    let result = tokio::time::timeout(Duration::from_secs(90), exercise(&config)).await;
    FirecrackerProvider::new(config.clone(), launcher(&config)?)
        .recover_stale()
        .await
        .context("post-test recovery failed")?;
    result.context("lifecycle example exceeded 90 seconds")?
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    let _ = std::io::Write::write_all(&mut std::io::stderr(), b"e2e_lifecycle needs Linux with /dev/kvm\n");
}
