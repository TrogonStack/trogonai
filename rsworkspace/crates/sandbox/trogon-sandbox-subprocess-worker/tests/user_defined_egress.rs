#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use trogon_sandbox_contract::{
    AllowRange, DomainRule, DomainRuleAction, HostMountAccess, LeaseId, LeaseRequest, LeaseRequestKey, NetworkPolicy,
    OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};
use trogon_sandbox_contract::{DeniedDomain, DomainPattern};
use trogon_sandbox_subprocess_worker::{FuseConfig, IsolationBackendKind, SubprocessWorker, WorkerConfig};
use trogon_std::env::{ReadEnv as _, SystemEnv};

const UPSTREAM_BODY: &str = "upstream-ok";
const ALLOWED_HOST: &str = "localhost";
const DENIED_HOST: &str = "denied.localhost";

const PROBE: &str = r#"
import base64, errno, os, socket, sys
from urllib.parse import urlparse

upstream_port = int(os.environ["UPSTREAM_PORT"])
proxy = urlparse(os.environ["HTTPS_PROXY"])
auth = base64.b64encode(("%s:%s" % (proxy.username, proxy.password)).encode()).decode()
report = []

def attempt(name, fn):
    try:
        report.append((name, fn()))
    except OSError as e:
        report.append((name, errno.errorcode.get(e.errno, str(e.errno))))
    except Exception as e:
        report.append((name, type(e).__name__))

def through_proxy(host, port):
    s = socket.create_connection((proxy.hostname, proxy.port), timeout=10)
    header = "CONNECT %s:%d HTTP/1.1\r\nHost: %s:%d\r\nProxy-Authorization: Basic %s\r\n\r\n" % (host, port, host, port, auth)
    s.sendall(header.encode())
    status = s.recv(4096).decode(errors="replace").split("\r\n")[0]
    if " 200" not in status:
        return status.replace(" ", "_") or "empty"
    s.sendall(b"GET / HTTP/1.0\r\n\r\n")
    body = b""
    while True:
        chunk = s.recv(4096)
        if not chunk:
            break
        body += chunk
    return "200:" + body.decode(errors="replace").strip().splitlines()[-1]

def direct(host, port):
    s = socket.create_connection((host, port), timeout=3)
    s.close()
    return "connected"

attempt("allowed", lambda: through_proxy("localhost", upstream_port))
attempt("denied_host", lambda: through_proxy("denied.localhost", upstream_port))
attempt("unlisted_ip", lambda: through_proxy("127.0.0.2", upstream_port))
attempt("metadata", lambda: through_proxy("169.254.169.254", 80))
attempt("direct_upstream", lambda: direct("127.0.0.1", upstream_port))
attempt("direct_metadata", lambda: direct("169.254.169.254", 80))
attempt("direct_elsewhere", lambda: direct("192.0.2.1", 80))

with open("/workspace/probe.txt", "w") as f:
    for name, value in report:
        f.write("%s %s\n" % (name, value))
"#;

fn tools_available() -> bool {
    ["bwrap", "python3", "bash"]
        .iter()
        .all(|tool| which::which(tool).is_ok())
}

fn target_profile_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    Ok(exe
        .parent()
        .and_then(Path::parent)
        .ok_or("target profile directory")?
        .to_path_buf())
}

fn proxy_binary() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = SystemEnv.var("SANDBOX_EGRESS_PROXYD") {
        return Ok(PathBuf::from(path));
    }
    let profile_dir = target_profile_dir()?;
    let candidate = profile_dir.join("trogon-sandbox-egress-proxyd");
    if !candidate.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-p",
                "trogon-sandbox-egress-proxy",
                "--bin",
                "trogon-sandbox-egress-proxyd",
            ])
            .status()?;
        assert!(status.success(), "building trogon-sandbox-egress-proxyd failed");
    }
    Ok(candidate)
}

async fn upstream() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{UPSTREAM_BODY}",
                    UPSTREAM_BODY.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    Ok(port)
}

fn policy(marker: &str) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new(ALLOWED_HOST)?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![AllowRange::new("127.0.0.1/32")?],
        deny_ranges: vec![],
        denied_domains: vec![DeniedDomain::new(DENIED_HOST)?, DeniedDomain::new(marker)?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn marker() -> Result<String, Box<dyn std::error::Error>> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    Ok(format!("m{nanos}-{}.test", std::process::id()))
}

fn worker(workspace_root: &Path) -> Result<SubprocessWorker, Box<dyn std::error::Error>> {
    Ok(SubprocessWorker::new(WorkerConfig {
        agent_binary_default: which::which("bash")?,
        workspace_root: workspace_root.to_path_buf(),
        isolation_backend: IsolationBackendKind::Bubblewrap,
        bwrap_binary_path: None,
        egress_proxy_binary_path: Some(proxy_binary()?),
        relay_binary_path: Some(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"))),
        fuse: FuseConfig {
            enabled: false,
            mount_s3_binary: PathBuf::from("mount-s3"),
            region: String::new(),
            endpoint: None,
            allow_other: false,
        },
    })?)
}

fn lease_request(
    workspace: &Path,
    policy: NetworkPolicy,
    env: &[(&str, String)],
) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef {
                registry: String::new(),
                repository: which::which("bash")?.to_string_lossy().into_owned(),
                tag: None,
                digest: None,
            },
            pull_secret: None,
        },
        workspace: WorkspaceSource::HostMount {
            path: workspace.to_string_lossy().into_owned(),
            access: HostMountAccess::ReadWrite,
        },
        env: env.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        cpu_seconds: NonZeroU32::new(30).ok_or("zero cpu")?,
        memory_mib: NonZeroU32::new(512).ok_or("zero memory")?,
        snapshot_out: None,
        network_policy: Some(policy),
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
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("t")?,
        provider_pref: vec![ProviderKind::Subprocess],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("k")?,
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
    })
}

fn pid_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

async fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + limit;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn lease_proxy_pids(marker: &str) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    Ok(std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
                return false;
            };
            environ
                .split(|byte| *byte == 0)
                .filter_map(|entry| std::str::from_utf8(entry).ok())
                .filter_map(|entry| entry.strip_prefix("EGRESS_PROXY_POLICY_FILE="))
                .any(|path| std::fs::read_to_string(path).is_ok_and(|policy| policy.contains(marker)))
        })
        .collect())
}

async fn run_probe() -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let port = upstream().await?;
    std::fs::write(workspace.path().join("probe.py"), PROBE)?;
    std::fs::write(workspace.path().join("run.sh"), "python3 /workspace/probe.py\nexit 0\n")?;

    let worker = worker(root.path())?;
    let request = lease_request(
        workspace.path(),
        policy(&marker()?)?,
        &[
            ("BASH_ENV", "/workspace/run.sh".to_string()),
            ("UPSTREAM_PORT", port.to_string()),
        ],
    )?;
    worker.launch_lease(&request, &LeaseId::new("probe")).await?;

    let report_path = workspace.path().join("probe.txt");
    wait_until("the probe report", Duration::from_secs(60), || report_path.exists()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(std::fs::read_to_string(report_path)?
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

#[tokio::test]
async fn a_user_defined_lease_reaches_only_what_its_policy_allows() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let r = run_probe().await?;
    let got = |key: &str| r.get(key).map(String::as_str).unwrap_or("<missing>");
    assert_eq!(got("allowed"), format!("200:{UPSTREAM_BODY}"), "{r:?}");
    assert!(got("denied_host").contains("403"), "{r:?}");
    assert!(got("unlisted_ip").contains("403"), "{r:?}");
    assert!(got("metadata").contains("403"), "{r:?}");
    assert_ne!(got("direct_upstream"), "connected", "{r:?}");
    assert_ne!(got("direct_metadata"), "connected", "{r:?}");
    assert_ne!(got("direct_elsewhere"), "connected", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn the_lease_ends_when_its_proxy_dies() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    std::fs::write(
        workspace.path().join("run.sh"),
        "touch /workspace/started\nsleep 300\nexit 0\n",
    )?;
    let marker = marker()?;
    let request = lease_request(
        workspace.path(),
        policy(&marker)?,
        &[("BASH_ENV", "/workspace/run.sh".to_string())],
    )?;

    let pid = worker(root.path())?
        .launch_lease(&request, &LeaseId::new("fail-closed"))
        .await?;
    wait_until("the payload to start", Duration::from_secs(30), || {
        workspace.path().join("started").exists()
    })
    .await;

    let proxies = lease_proxy_pids(&marker)?;
    assert_eq!(proxies.len(), 1, "one proxy per lease: {proxies:?}");
    let killed = unsafe { nix::libc::kill(proxies[0], nix::libc::SIGKILL) };
    assert_eq!(killed, 0, "kill the proxy");

    wait_until("the lease to end after its proxy died", Duration::from_secs(20), || {
        !pid_exists(pid)
    })
    .await;
    Ok(())
}

#[tokio::test]
async fn the_proxy_is_torn_down_with_its_lease() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    std::fs::write(
        workspace.path().join("run.sh"),
        "touch /workspace/started\nsleep 300\nexit 0\n",
    )?;
    let marker = marker()?;
    let request = lease_request(
        workspace.path(),
        policy(&marker)?,
        &[("BASH_ENV", "/workspace/run.sh".to_string())],
    )?;

    let worker = worker(root.path())?;
    let lease = LeaseId::new("teardown");
    let pid = worker.launch_lease(&request, &lease).await?;
    wait_until("the payload to start", Duration::from_secs(30), || {
        workspace.path().join("started").exists()
    })
    .await;
    assert_eq!(lease_proxy_pids(&marker)?.len(), 1);

    let killed = unsafe { nix::libc::kill(pid as i32, nix::libc::SIGKILL) };
    assert_eq!(killed, 0, "kill the lease");

    wait_until("the proxy to go away with its lease", Duration::from_secs(20), || {
        lease_proxy_pids(&marker).is_ok_and(|pids| pids.is_empty())
    })
    .await;
    Ok(())
}
