//! Leases a real microVM, runs `uname -a` through the run_command path, and cancels.
//!
//! Needs a Linux host with /dev/kvm, a `firecracker` binary, and a kernel and rootfs the
//! guest agent is installed in. Configured through the environment:
//!
//! - `E2E_FIRECRACKER_BINARY` (default `firecracker`)
//! - `E2E_KERNEL` and `E2E_ROOTFS` (required)
//! - `E2E_STATE_DIR` (default `/tmp/fc-e2e`)
//! - `E2E_IMAGE` (default `guest:e2e`, only a label for the rootfs)

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use std::io::Write;

use anyhow::Context;
use trogon_sandbox_contract::{
    LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind,
    RunCommandRequest, RunCommandResponse, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, TenantId,
    WorkspaceSource,
};
use trogon_sandbox_firecracker_worker::FirecrackerProvider;
use trogon_sandbox_firecracker_worker::agent::DiscardChunks;
use trogon_sandbox_firecracker_worker::config::{
    BaseRootfs, FirecrackerBinary, FirecrackerConfig, KernelImage, StateDir,
};
use trogon_sandbox_firecracker_worker::process::DirectLauncher;
use trogon_sandbox_protocol::Provider;
use trogon_std::env::{ReadEnv, SystemEnv};

fn env(vars: &impl ReadEnv, name: &str) -> anyhow::Result<String> {
    vars.var(name).map_err(|_| anyhow::anyhow!("{name} must be set"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let vars = SystemEnv;
    let mut stdout = std::io::stdout();
    let image = vars.var("E2E_IMAGE").unwrap_or_else(|_| "guest:e2e".to_string());
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new(
            vars.var("E2E_FIRECRACKER_BINARY")
                .unwrap_or_else(|_| "firecracker".into()),
        ),
        KernelImage::new(PathBuf::from(env(&vars, "E2E_KERNEL")?)),
        BaseRootfs::new(PathBuf::from(env(&vars, "E2E_ROOTFS")?)),
        StateDir::new(vars.var("E2E_STATE_DIR").unwrap_or_else(|_| "/tmp/fc-e2e".to_string())),
    );
    config.default_image = OciImageRef::parse_unqualified(&image);

    let launcher = Arc::new(DirectLauncher::new(config.binary.clone()));
    let provider = FirecrackerProvider::new(config, launcher);
    provider.clean_stale().await;

    let lease_id = LeaseId::new(format!("e2e{}", std::process::id()));
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(&image),
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
    })?;
    let request = LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("e2e")?,
        provider_pref: vec![ProviderKind::Firecracker],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("e2e")?,
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
    };

    let lease = provider.lease(&request, &lease_id).await?;
    writeln!(stdout, "leased {}", lease.sandbox_id)?;

    let outcome = async {
        let agent = provider
            .agent_for(&lease_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("the lease vanished after it was granted"))?;
        let run: RunCommandRequest = serde_json::from_str(&format!(
            r#"{{"lease_id":"{}","command":["uname","-a"],"timeout_seconds":30}}"#,
            lease_id.as_str()
        ))?;
        anyhow::Ok(agent.exec(&run, &DiscardChunks).await?)
    }
    .await;

    provider.cancel(&lease_id).await?;
    writeln!(stdout, "cancelled {lease_id}")?;

    match outcome? {
        RunCommandResponse::Completed(output) => {
            write!(stdout, "{}", String::from_utf8_lossy(&output.stdout))?;
            anyhow::ensure!(
                output.exit_code == 0,
                "uname exited {}: {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(())
        }
        other => anyhow::bail!("run_command did not complete: {other:?}"),
    }
}
