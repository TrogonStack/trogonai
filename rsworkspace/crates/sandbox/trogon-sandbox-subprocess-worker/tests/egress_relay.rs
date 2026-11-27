#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use trogon_sandbox_contract::{AllowRange, NetworkPolicy};
use trogon_sandbox_contract::{DirectoryMarker, LiteralPath};
use trogon_sandbox_contract::{FsPathEntry, FsPolicy};
use trogon_sandbox_subprocess_worker::fs_violation::FsDenialSink;
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::relay::{RELAY_EXIT_CODE, RelayEgress};
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::lease_proxy::LeaseProxy;
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};
use trogon_std::env::{ReadEnv as _, SystemEnv};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

const UPSTREAM_BODY: &str = "upstream-ok";

const PROBE: &str = r#"
import base64, errno, os, socket, sys

upstream_port = int(sys.argv[1])
token = sys.argv[2]
auth = base64.b64encode(("x:" + token).encode()).decode()
report = []

def attempt(name, fn):
    try:
        report.append((name, fn()))
    except OSError as e:
        report.append((name, errno.errorcode.get(e.errno, str(e.errno))))
    except Exception as e:
        report.append((name, type(e).__name__))

def through_relay(host, port, with_auth=True):
    s = socket.create_connection(("127.0.0.1", 3128), timeout=10)
    header = "CONNECT %s:%d HTTP/1.1\r\nHost: %s:%d\r\n" % (host, port, host, port)
    if with_auth:
        header += "Proxy-Authorization: Basic %s\r\n" % auth
    s.sendall((header + "\r\n").encode())
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

def unix_socket():
    socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    return "created"

attempt("allowed", lambda: through_relay("127.0.0.1", upstream_port))
attempt("denied_destination", lambda: through_relay("127.0.0.2", upstream_port))
attempt("missing_token", lambda: through_relay("127.0.0.1", upstream_port, False))
attempt("direct_upstream", lambda: direct("127.0.0.1", upstream_port))
attempt("direct_elsewhere", lambda: direct("192.0.2.1", 80))
attempt("unix_socket", unix_socket)
attempt("supervisor_environ", lambda: open("/proc/%d/environ" % os.getppid(), "rb").read() and "readable")

with open("/workspace/probe.txt", "w") as f:
    for name, value in report:
        f.write("%s %s\n" % (name, value))
"#;

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn proxy_binary() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = SystemEnv.var("SANDBOX_EGRESS_PROXYD") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe()?;
    let profile_dir = exe.parent().and_then(Path::parent).ok_or("target profile directory")?;
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

fn relay_egress(proxy: &LeaseProxy) -> RelayEgress {
    RelayEgress {
        proxy_socket: proxy.socket().to_path_buf(),
        relay_binary: PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker")),
    }
}

fn allow_loopback_policy() -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("127.0.0.1/32")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn fs_policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(FsPolicy::new(
        vec![literal("/workspace/secret.txt")?],
        Vec::new(),
        vec![],
        Vec::new(),
        vec![literal("/workspace")?],
        vec![],
    )?)
}

fn spec(ws: &Path, binary: PathBuf, args: Vec<String>, relay: RelayEgress, fs_policy: Option<FsPolicy>) -> LaunchSpec {
    LaunchSpec {
        binary,
        args,
        env: vec![],
        workspace_host_path: ws.to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::Relay(relay),
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy,
        platform: Default::default(),
        fs_denials: None,
    }
}

async fn run_probe(fs_policy: Option<FsPolicy>) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    run_script(PROBE, fs_policy).await
}

async fn run_script(
    script: &str,
    fs_policy: Option<FsPolicy>,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let ws = tempfile::tempdir()?;
    let port = upstream().await?;
    let proxy = LeaseProxy::spawn(&proxy_binary()?, &allow_loopback_policy()?).await?;
    let relay = relay_egress(&proxy);
    let backend = BubblewrapBackend::new(which::which("bwrap")?);
    let launch = spec(
        ws.path(),
        which::which("python3")?,
        vec![
            "-c".to_string(),
            script.to_string(),
            port.to_string(),
            proxy.token().expose().to_string(),
        ],
        relay,
        fs_policy,
    );

    let launched = backend.launch(&launch).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await??;
    let report = std::fs::read_to_string(ws.path().join("probe.txt")).unwrap_or_default();
    assert!(
        output.status.success(),
        "probe exited with {}: {report}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(report
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

fn assert_probe(r: &BTreeMap<String, String>) {
    let got = |key: &str| r.get(key).map(String::as_str).unwrap_or("<missing>");
    assert_eq!(got("allowed"), format!("200:{UPSTREAM_BODY}"), "{r:?}");
    assert!(got("denied_destination").contains("403"), "{r:?}");
    assert!(got("missing_token").contains("407"), "{r:?}");
    assert_ne!(got("direct_upstream"), "connected", "{r:?}");
    assert_ne!(got("direct_elsewhere"), "connected", "{r:?}");
    assert_eq!(got("unix_socket"), "EPERM", "{r:?}");
    assert_eq!(got("supervisor_environ"), "EACCES", "{r:?}");
}

#[tokio::test]
async fn relay_reaches_the_allowed_upstream_and_nothing_else() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_probe(&run_probe(None).await?);
    Ok(())
}

const LISTEN_PROBE: &str = r#"
import base64, socket, sys, threading

upstream_port = int(sys.argv[1])
auth = base64.b64encode(("x:" + sys.argv[2]).encode()).decode()
report = []

def through(family, host):
    s = socket.socket(family, socket.SOCK_STREAM)
    s.settimeout(20)
    s.connect((host, 3128))
    s.sendall(("CONNECT 127.0.0.1:%d HTTP/1.1\r\nHost: 127.0.0.1:%d\r\nProxy-Authorization: Basic %s\r\n\r\n" % (upstream_port, upstream_port, auth)).encode())
    status = s.recv(4096).decode(errors="replace").split("\r\n")[0]
    s.close()
    return " 200" in status

try:
    report.append(("v4", "ok" if through(socket.AF_INET, "127.0.0.1") else "refused"))
except OSError as e:
    report.append(("v4", "error"))
try:
    has6 = "00000000000000000000000000000001" in open("/proc/net/if_inet6").read()
except OSError:
    has6 = False
report.append(("has_v6", "yes" if has6 else "no"))
try:
    report.append(("v6", "ok" if through(socket.AF_INET6, "::1") else "refused"))
except OSError as e:
    report.append(("v6", "unreachable"))

results = []
def one():
    try:
        results.append(through(socket.AF_INET, "127.0.0.1"))
    except OSError:
        results.append(False)
threads = [threading.Thread(target=one) for _ in range(64)]
for t in threads: t.start()
for t in threads: t.join()
report.append(("burst", "%d/64" % sum(results)))

with open("/workspace/probe.txt", "w") as f:
    for name, value in report:
        f.write("%s %s\n" % (name, value))
"#;

#[tokio::test]
async fn the_relay_serves_loopback_over_both_families_and_takes_a_burst() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let r = run_script(LISTEN_PROBE, None).await?;
    assert_eq!(r.get("v4").map(String::as_str), Some("ok"), "{r:?}");
    assert_eq!(r.get("burst").map(String::as_str), Some("64/64"), "{r:?}");
    if r.get("has_v6").map(String::as_str) == Some("yes") {
        assert_eq!(r.get("v6").map(String::as_str), Some("ok"), "{r:?}");
    }
    Ok(())
}

#[tokio::test]
async fn the_relay_also_listens_on_the_socks_port() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let r = run_script(&LISTEN_PROBE.replace("3128", "1080"), None).await?;
    assert_eq!(r.get("v4").map(String::as_str), Some("ok"), "{r:?}");
    assert_eq!(r.get("burst").map(String::as_str), Some("64/64"), "{r:?}");
    if r.get("has_v6").map(String::as_str) == Some("yes") {
        assert_eq!(r.get("v6").map(String::as_str), Some("ok"), "{r:?}");
    }
    Ok(())
}

#[tokio::test]
async fn relay_holds_with_an_fs_policy_mount_plan() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_probe(&run_probe(Some(fs_policy()?)).await?);
    Ok(())
}

fn parent_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

fn is_descendant_of(pid: i32, ancestor: i32) -> bool {
    let mut current = pid;
    while let Some(parent) = parent_of(current) {
        if parent == ancestor {
            return true;
        }
        if parent <= 1 {
            return false;
        }
        current = parent;
    }
    false
}

fn relay_of(lease_root: i32) -> Option<i32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|comm| comm.trim() == "egress-relay")
        })
        .find(|pid| is_descendant_of(*pid, lease_root))
}

#[tokio::test]
async fn relay_death_ends_the_lease() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let proxy = LeaseProxy::spawn(&proxy_binary()?, &allow_loopback_policy()?).await?;
    let relay = relay_egress(&proxy);
    let backend = BubblewrapBackend::new(which::which("bwrap")?);
    let launch = spec(ws.path(), which::which("sleep")?, vec!["300".to_string()], relay, None);

    let launched = backend.launch(&launch).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let mut child = child;

    let lease_root = child.id().ok_or("lease pid")? as i32;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let relay_pid = loop {
        if let Some(pid) = relay_of(lease_root) {
            break pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the relay process never appeared"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let killed = unsafe { nix::libc::kill(relay_pid, nix::libc::SIGKILL) };
    assert_eq!(killed, 0, "kill the relay");

    let status = tokio::time::timeout(Duration::from_secs(15), child.wait()).await??;
    assert!(!status.success(), "lease ended cleanly: {status}");
    assert_eq!(status.code(), Some(RELAY_EXIT_CODE), "{status}");
    Ok(())
}

#[tokio::test]
async fn the_lease_proxy_is_private_to_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let proxy = LeaseProxy::spawn(&proxy_binary()?, &allow_loopback_policy()?).await?;
    let socket_mode = std::fs::metadata(proxy.socket())?.permissions().mode() & 0o777;
    let dir_mode = std::fs::metadata(proxy.socket().parent().ok_or("socket directory")?)?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600);
    assert_eq!(dir_mode & 0o077, 0);
    Ok(())
}

#[tokio::test]
async fn denied_writes_are_reported_while_the_payload_runs_behind_the_relay() -> Result<(), Box<dyn std::error::Error>>
{
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let proxy = LeaseProxy::spawn(&proxy_binary()?, &allow_loopback_policy()?).await?;
    let (denials, mut receiver) = FsDenialSink::channel();
    let mut launch = spec(
        ws.path(),
        which::which("sh")?,
        vec![
            "-c".to_string(),
            "echo x > /usr/denied.txt 2>/dev/null; echo ok > /workspace/fine.txt; exit 4".to_string(),
        ],
        relay_egress(&proxy),
        Some(fs_policy()?),
    );
    launch.fs_denials = Some(denials);
    let backend = BubblewrapBackend::new(which::which("bwrap")?)
        .with_observer(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker")));

    let launched = backend.launch(&launch).await?;
    drop(launch);
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    drop(launched);
    assert_eq!(output.status.code(), Some(4));
    assert!(ws.path().join("fine.txt").exists());

    let denial = receiver.recv().await.ok_or("a denial")?;
    assert_eq!(denial.path.as_str(), "/usr/denied.txt");
    assert_eq!(denial.operation, trogon_sandbox_contract::FsOperation::Create);
    assert!(receiver.recv().await.is_none(), "the allowed write is not reported");
    Ok(())
}
