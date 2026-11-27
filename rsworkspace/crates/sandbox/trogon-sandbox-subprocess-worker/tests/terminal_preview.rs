#![cfg(target_os = "linux")]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use trogon_sandbox_contract::{
    GetSandboxDomainResponse, LeaseId, LeaseRequest, TerminalId, TerminalOutput, TerminalResponse,
};
use trogon_sandbox_protocol::{Provider, subjects, terminal_output_bytes};
use trogon_sandbox_subprocess_worker::port_forward::PortForwardRegistry;
use trogon_sandbox_subprocess_worker::terminal::{FrameSink, SubprocessTerminals};
use trogon_sandbox_subprocess_worker::{FuseConfig, IsolationBackendKind, SubprocessWorker, WorkerConfig};
use trogon_std::env::SystemEnv;

type TestResult = Result<(), Box<dyn Error>>;

/// A check of the scenario failed.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct CheckFailedError(String);

fn ensure(condition: bool, message: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(CheckFailedError(message.into()).into())
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const PRIMARY_PORT: u16 = 18080;
const TERMINAL_PORT: u16 = 18081;
const TERMINAL: &str = "preview-shell";
const SERVER: &str = r#"import socket, sys
listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(('127.0.0.1', int(sys.argv[1])))
listener.listen(8)
print('terminal-preview-ready', flush=True)
while True:
    client, _ = listener.accept()
    data = bytearray()
    while True:
        chunk = client.recv(4096)
        if not chunk:
            break
        data.extend(chunk)
    with open('/workspace/' + sys.argv[2] + '.requests', 'ab') as requests:
        requests.write(data + b'\n')
    client.sendall(sys.argv[2].encode() + b':' + data)
    client.close()
"#;

#[derive(Default)]
struct Frames {
    terminal: Mutex<Vec<TerminalOutput>>,
    changed: tokio::sync::Notify,
}

#[async_trait]
impl FrameSink for Frames {
    async fn publish(&self, subject: String, payload: Bytes) {
        if let Ok(id) = TerminalId::new(TERMINAL)
            && subject == subjects::terminal_output(&id)
            && let Ok(frame) = serde_json::from_slice(&payload)
        {
            locked(&self.terminal).push(frame);
            self.changed.notify_waiters();
        }
    }
}

impl Frames {
    async fn wait_ready(&self) -> TestResult {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let changed = self.changed.notified();
                let output: Vec<u8> = locked(&self.terminal)
                    .iter()
                    .filter_map(terminal_output_bytes)
                    .flatten()
                    .collect();
                if String::from_utf8_lossy(&output).contains("terminal-preview-ready") {
                    return;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| CheckFailedError("the terminal server did not establish its listener".into()))?;
        Ok(())
    }
}

fn request(workspace: &Path, label: &str) -> Result<LeaseRequest, Box<dyn Error>> {
    let workspace = workspace.to_str().ok_or("workspace path is not UTF-8")?;
    Ok(serde_json::from_value(json!({
        "tenant": "terminal-preview-owner",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "", "repository": "/bin/bash" },
            },
            "workspace": { "host_mount": { "path": workspace, "readonly": false } },
            "network_policy": "deny_all",
            "cpu_seconds": 2,
            "memory_mib": 512,
            "env": { "BASH_ENV": "/workspace/primary.sh", "PREVIEW_LABEL": label },
            "mounts": [],
        },
        "ttl_seconds": 60,
        "idempotency_key": label,
    }))?)
}

/// A terminal request as it arrives on the wire: the operation's fields sit next to the ids.
fn terminal_request(lease: &LeaseId, op: Value) -> Vec<u8> {
    let mut body = json!({ "lease_id": lease.as_str(), "terminal_id": TERMINAL });
    if let (Some(fields), Value::Object(op)) = (body.as_object_mut(), op) {
        fields.extend(op);
    }
    body.to_string().into_bytes()
}

fn open_server(lease: &LeaseId, port: u16) -> Vec<u8> {
    terminal_request(
        lease,
        json!({
            "op": "open",
            "cols": 80,
            "rows": 24,
            "cmd": ["python3", "-u", "/workspace/server.py", port.to_string(), "terminal"],
        }),
    )
}

async fn response(endpoint: &str, expected: &[u8]) -> bool {
    exchange(endpoint, b"probe").await.as_deref() == Some(expected)
}

async fn exchange(endpoint: &str, request: &[u8]) -> Option<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut stream = TcpStream::connect(endpoint).await?;
        stream.write_all(request).await?;
        stream.shutdown().await?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>(bytes)
    })
    .await
    .ok()?
    .ok()
}

async fn endpoint(
    worker: &SubprocessWorker,
    forwards: &PortForwardRegistry,
    lease: &LeaseId,
    port: u16,
) -> Result<String, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let resolution = forwards.resolve(lease, port).await;
        if let GetSandboxDomainResponse::Available { endpoint, .. } = resolution {
            return Ok(endpoint);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CheckFailedError(format!(
                "advertised private-network preview did not become ready: {resolution:?}; {}",
                process_diagnostic(worker, lease).await
            ))
            .into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn process_diagnostic(worker: &SubprocessWorker, lease: &LeaseId) -> String {
    let Some(pid) = worker.lease_pid(lease).await else {
        return format!("lease {lease} is no longer held after its process exited");
    };
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
    let namespace = std::fs::read_link(format!("/proc/{pid}/ns/net"));
    format!("lease {lease} tracked PID {pid}: stat={stat:?}, network={namespace:?}")
}

async fn wait_response(worker: &SubprocessWorker, lease: &LeaseId, endpoint: &str, expected: &[u8]) -> TestResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if response(endpoint, expected).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CheckFailedError(format!(
                "the owned primary-process preview never answered; {}",
                process_diagnostic(worker, lease).await
            ))
            .into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn worker(
    root: &Path,
    forwards: &PortForwardRegistry,
    terminals: &Arc<SubprocessTerminals>,
) -> Result<SubprocessWorker, Box<dyn Error>> {
    Ok(SubprocessWorker::new(WorkerConfig {
        agent_binary_default: PathBuf::from("/bin/bash"),
        workspace_root: root.join("workspaces"),
        isolation_backend: IsolationBackendKind::Bubblewrap,
        bwrap_binary_path: None,
        egress_proxy_binary_path: None,
        relay_binary_path: None,
        fuse: FuseConfig {
            enabled: false,
            mount_s3_binary: "mount-s3".into(),
            region: String::new(),
            endpoint: None,
            allow_other: false,
        },
    })?
    .with_port_forwards(forwards.clone())
    .with_port_bridge_binary(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker")))
    .with_terminals(terminals.clone()))
}

/// Writes the server and primary-process scripts both leases share.
fn write_workspace(root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let workspace = root.join("shared-workspace");
    std::fs::create_dir(&workspace)?;
    std::fs::write(workspace.join("server.py"), SERVER)?;
    std::fs::write(
        workspace.join("primary.sh"),
        format!("exec python3 -u /workspace/server.py {PRIMARY_PORT} \"$PREVIEW_LABEL\"\n"),
    )?;
    Ok(workspace)
}

/// Cancels both owned leases and checks that every per-lease resource was retired.
async fn retire(
    worker: &SubprocessWorker,
    forwards: &PortForwardRegistry,
    terminals: &SubprocessTerminals,
    leases: [&LeaseId; 2],
) -> TestResult {
    let mut cleanup_errors = Vec::new();
    for owned in leases {
        if let Err(error) = worker.cancel(owned).await {
            cleanup_errors.push(error.to_string());
        }
    }
    ensure(
        cleanup_errors.is_empty(),
        format!("owned fixture cleanup remained pending: {cleanup_errors:?}"),
    )?;
    for owned in leases {
        ensure(
            !forwards.holds(owned).await && !terminals.holds(owned),
            "both owned lease resources must be retired before the behavioral assertion",
        )?;
    }
    Ok(())
}

fn require_tools() -> TestResult {
    which::which("bwrap").map_err(|_| CheckFailedError("strict native proof requires bubblewrap".into()))?;
    which::which("python3").map_err(|_| CheckFailedError("strict native proof requires Python".into()))?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "strict Linux proof: requires working bubblewrap user/network namespaces, Python and a container seccomp profile permitting the worker's setns helper"]
async fn a_terminal_server_is_reachable_through_its_own_leases_preview() -> TestResult {
    require_tools()?;
    let profile = trogon_sandbox_subprocess_worker::profile_for_backend(IsolationBackendKind::Bubblewrap);
    ensure(
        profile.isolation.supports_terminal.is_supported()
            && profile.isolation.supports_port_preview.is_supported()
            && profile.isolation.supports_expose_after_create.is_supported(),
        "this Linux backend must advertise the terminal and preview contract under test",
    )?;

    let root = tempfile::tempdir()?;
    let workspace = write_workspace(root.path())?;
    let frames = Arc::new(Frames::default());
    let terminals = SubprocessTerminals::new(frames.clone(), &SystemEnv);
    let forwards = PortForwardRegistry::new();
    let worker = worker(root.path(), &forwards, &terminals)?;
    let lease = LeaseId::new("owned-terminal-preview");
    let unrelated = LeaseId::new("unrelated-terminal-preview");
    let observed: Result<bool, Box<dyn Error>> = async {
        worker.lease(&request(&workspace, "primary")?, &lease).await?;
        worker.lease(&request(&workspace, "unrelated")?, &unrelated).await?;
        let primary = endpoint(&worker, &forwards, &lease, PRIMARY_PORT).await?;
        let sibling = endpoint(&worker, &forwards, &unrelated, PRIMARY_PORT).await?;
        wait_response(&worker, &lease, &primary, b"primary:probe").await?;
        wait_response(&worker, &unrelated, &sibling, b"unrelated:probe").await?;
        ensure(
            TcpStream::connect(("127.0.0.1", PRIMARY_PORT)).await.is_err(),
            "the primary server must not escape onto host loopback",
        )?;
        let opened = terminals.handle(&open_server(&lease, TERMINAL_PORT)).await;
        ensure(
            matches!(opened, Some(TerminalResponse::Opened { .. })),
            format!("the terminal must open through the public worker contract: {opened:?}"),
        )?;
        frames.wait_ready().await?;
        ensure(
            TcpStream::connect(("127.0.0.1", TERMINAL_PORT)).await.is_err(),
            "the terminal server must stay off host loopback",
        )?;
        let preview = endpoint(&worker, &forwards, &lease, TERMINAL_PORT).await?;
        let terminal_reached = response(&preview, b"terminal:probe").await;
        ensure(
            response(&primary, b"primary:probe").await,
            "the same lease's ordinary preview control must remain reachable",
        )?;
        ensure(
            response(&sibling, b"unrelated:probe").await,
            "preview must preserve the unrelated lease and its namespace",
        )?;
        Ok(terminal_reached)
    }
    .await;
    retire(&worker, &forwards, &terminals, [&lease, &unrelated]).await?;
    ensure(
        observed?,
        "advertised terminal preview did not reach the server; same-lease primary and unrelated-lease preview controls passed, host ports stayed isolated, and owned cleanup completed",
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "strict Linux proof: requires working bubblewrap user/network namespaces, Python and a container seccomp profile permitting the worker's setns helper"]
async fn an_ambiguous_owned_namespace_port_is_refused_until_the_terminal_closes() -> TestResult {
    require_tools()?;
    let root = tempfile::tempdir()?;
    let workspace = write_workspace(root.path())?;
    let frames = Arc::new(Frames::default());
    let terminals = SubprocessTerminals::new(frames.clone(), &SystemEnv);
    let forwards = PortForwardRegistry::new();
    let worker = worker(root.path(), &forwards, &terminals)?;
    let lease = LeaseId::new("ambiguous-terminal-preview");
    let unrelated = LeaseId::new("ambiguous-unrelated-preview");
    let observed: Result<(bool, bool), Box<dyn Error>> = async {
        worker.lease(&request(&workspace, "primary")?, &lease).await?;
        worker.lease(&request(&workspace, "unrelated")?, &unrelated).await?;
        let primary = endpoint(&worker, &forwards, &lease, PRIMARY_PORT).await?;
        let sibling = endpoint(&worker, &forwards, &unrelated, PRIMARY_PORT).await?;
        wait_response(&worker, &lease, &primary, b"primary:probe").await?;
        wait_response(&worker, &unrelated, &sibling, b"unrelated:probe").await?;
        let opened = terminals.handle(&open_server(&lease, PRIMARY_PORT)).await;
        ensure(
            matches!(opened, Some(TerminalResponse::Opened { .. })),
            format!("the second owned listener must open through the terminal: {opened:?}"),
        )?;
        frames.wait_ready().await?;
        let delivered = exchange(&primary, b"ambiguous-probe").await;
        let closed = terminals
            .handle(&terminal_request(&lease, json!({ "op": "close" })))
            .await;
        ensure(closed == Some(TerminalResponse::Done), "the owned terminal must close")?;
        wait_response(&worker, &lease, &primary, b"primary:probe").await?;
        ensure(
            response(&sibling, b"unrelated:probe").await,
            "the unrelated lease's preview must survive ambiguity and close",
        )?;
        let mut request_delivered = false;
        for label in ["primary", "terminal"] {
            let requests = std::fs::read(workspace.join(format!("{label}.requests"))).unwrap_or_default();
            request_delivered |= requests
                .windows(b"ambiguous-probe".len())
                .any(|window| window == b"ambiguous-probe");
        }
        Ok((delivered.is_none_or(|bytes| bytes.is_empty()), request_delivered))
    }
    .await;
    retire(&worker, &forwards, &terminals, [&lease, &unrelated]).await?;
    let (refused, request_delivered) = observed?;
    ensure(
        refused && !request_delivered,
        "preview chose one of two owned namespaces and delivered client bytes before the terminal closed; primary recovery, unrelated preview and owned cleanup controls passed",
    )
}
