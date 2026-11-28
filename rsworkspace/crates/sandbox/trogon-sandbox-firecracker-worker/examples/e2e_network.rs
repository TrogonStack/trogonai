//! Validates per-lease host-enforced proxy networking on a Linux KVM host.
//!
//! Needs a Linux host with /dev/kvm, a `firecracker` binary, and a kernel and rootfs the
//! guest agent is installed in. Configured through the environment:
//!
//! - `E2E_FIRECRACKER_BINARY` (default `firecracker`)
//! - `E2E_KERNEL` and `E2E_ROOTFS` (required)
//! - `E2E_STATE_DIR` (default `/tmp/fc-e2e`)
//! - `E2E_IMAGE` (default `guest:e2e`, only a label for the rootfs)

#[cfg(target_os = "linux")]
use anyhow::Context;
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::num::NonZeroU32;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(target_os = "linux")]
use trogon_sandbox_contract::{
    BootMode, CommandEnv, CommandExecution, LaunchCancellation, LeaseId, LeaseRequest, LeaseRequestKey, NetworkPolicy,
    OciImageRef, ProviderKind, RunCommandRequest, RunCommandResponse, SandboxImage, SandboxSpec, SandboxSpecParts,
    SandboxTags, TenantId, WorkspaceSource,
};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::FirecrackerProvider;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::agent::DiscardChunks;
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::config::{
    BaseRootfs, FirecrackerBinary, FirecrackerConfig, KernelImage, StateDir,
};
#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::process::DirectLauncher;
#[cfg(target_os = "linux")]
use trogon_sandbox_protocol::Provider;
#[cfg(target_os = "linux")]
use trogon_std::env::{ReadEnv, SystemEnv};

#[cfg(target_os = "linux")]
fn env(name: &str) -> anyhow::Result<String> {
    SystemEnv.var(name).map_err(|_| anyhow::anyhow!("{name} must be set"))
}

#[cfg(target_os = "linux")]
async fn run() -> anyhow::Result<()> {
    let image = SystemEnv.var("E2E_IMAGE").unwrap_or_else(|_| "guest:e2e".to_string());
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new(
            SystemEnv
                .var("E2E_FIRECRACKER_BINARY")
                .unwrap_or_else(|_| "firecracker".into()),
        ),
        KernelImage::new(PathBuf::from(env("E2E_KERNEL")?)),
        BaseRootfs::new(PathBuf::from(env("E2E_ROOTFS")?)),
        StateDir::new(
            SystemEnv
                .var("E2E_STATE_DIR")
                .unwrap_or_else(|_| "/tmp/fc-e2e".to_string()),
        ),
    );
    config.default_image = OciImageRef::parse_unqualified(&image);
    config.network = Some(trogon_sandbox_firecracker_worker::network::NetworkConfig {
        host_links: SystemEnv
            .var("E2E_HOST_LINKS")
            .unwrap_or_else(|_| "10.231.0.0/24".into())
            .parse()?,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let target_port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buffer = [0; 8192];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nfc-network-ok")
                    .await;
            });
        }
    });

    let launcher = Arc::new(DirectLauncher::new(config.binary.clone()));
    let provider = FirecrackerProvider::new(config, launcher);
    provider.clean_stale().await;

    let lease_id = LeaseId::new(format!("e2e{}", std::process::id()));
    let request = LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("e2e")?,
        provider_pref: vec![ProviderKind::Firecracker],
        spec: SandboxSpec::new(SandboxSpecParts {
            image: SandboxImage::Registry {
                reference: OciImageRef::parse_unqualified(&image),
                pull_secret: None,
            },
            workspace: WorkspaceSource::empty(),
            env: BTreeMap::new(),
            cpu_seconds: NonZeroU32::new(60).context("cpu seconds")?,
            memory_mib: NonZeroU32::new(256).context("memory")?,
            snapshot_out: None,
            network_policy: Some(serde_json::from_str::<NetworkPolicy>(
                r#"{"user_defined": {"rules": [], "allow_ranges": ["127.0.0.1/32"], "deny_ranges": [], "opaque_tunnels": "allow"}}"#,
            )?),
            mounts: vec![],
            drives: vec![],
            exposed_ports: vec![],
            boot_mode: BootMode::default(),
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
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("e2e")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: Default::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: Default::default(),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    };

    let lease = provider.lease(&request, &lease_id).await?;
    writeln!(std::io::stdout(), "leased {}", lease.sandbox_id)?;

    let outcome = async {
        let agent = provider
            .agent_for(&lease_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("the lease vanished after it was granted"))?;
        let run = RunCommandRequest {
            require_launch_cancellation: LaunchCancellation::default(),
            lease_id: lease_id.clone(),
            command: vec![
                "python3".to_string(),
                "-c".to_string(),
                format!(
                    r#"
import base64, http.client, os, socket, urllib.parse
proxy = urllib.parse.urlsplit(os.environ['HTTP_PROXY'])
connection = http.client.HTTPConnection(proxy.hostname, proxy.port, timeout=5)
auth = base64.b64encode((proxy.username + ':' + proxy.password).encode()).decode()
connection.request('GET', 'http://127.0.0.1:{target_port}/', headers={{'Proxy-Authorization': 'Basic ' + auth}})
response = connection.getresponse()
assert response.status == 200, response.status
assert response.read() == b'fc-network-ok'
connection.close()
for address, port in [(proxy.hostname, {target_port}), ('1.1.1.1', 80)]:
    try:
        socket.create_connection((address, port), timeout=2).close()
    except OSError:
        pass
    else:
        raise AssertionError('guest bypassed host proxy restrictions')
print('proxy request succeeded; direct guest egress denied')
"#
                ),
            ],
            cwd: None,
            env: CommandEnv::default(),
            timeout_seconds: 30,
            stream_output: Default::default(),
            run_as_user: None,
            output_retention: Default::default(),
            max_output_bytes: None,
            redact_values: Default::default(),
            launch_key: None,
            execution: CommandExecution::default(),
        };
        anyhow::Ok(agent.exec(&run, &DiscardChunks).await?)
    }
    .await;

    provider.cancel(&lease_id).await?;
    server.abort();
    writeln!(std::io::stdout(), "cancelled {}", lease_id)?;

    match outcome? {
        RunCommandResponse::Completed(output) => {
            write!(std::io::stdout(), "{}", String::from_utf8_lossy(&output.stdout))?;
            anyhow::ensure!(
                output.exit_code == 0,
                "network probe exited {}: {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(())
        }
        other => anyhow::bail!("run_command did not complete: {other:?}"),
    }
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    let _ = std::io::Write::write_all(&mut std::io::stderr(), b"e2e_network needs Linux with /dev/kvm\n");
}
