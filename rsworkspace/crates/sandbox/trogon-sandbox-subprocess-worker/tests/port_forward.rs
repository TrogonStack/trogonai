//! A port a lease listens on is reachable through the worker's forward, even
//! though the lease's loopback is a network namespace of its own.

#![cfg(target_os = "linux")]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseError, LeaseId, LeaseRequest};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_subprocess_worker::port_forward::PortForwardRegistry;
use trogon_sandbox_subprocess_worker::{FuseConfig, IsolationBackendKind, SubprocessWorker, WorkerConfig};

type TestResult = Result<(), Box<dyn Error>>;

const LEASE_PORT: u16 = 18080;

/// Answers `pong:<request>` to whatever a client sends before it half-closes.
const SERVER: &str = r#"#!/bin/sh
exec python3 -c '
import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 18080))
s.listen(8)
while True:
    c, _ = s.accept()
    data = b""
    while True:
        chunk = c.recv(4096)
        if not chunk:
            break
        data += chunk
    c.sendall(b"pong:" + data)
    c.close()
'
"#;

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn lease_request(agent: &Path, workspace: &Path) -> Result<LeaseRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "tenant": "t",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "", "repository": agent.to_string_lossy() },
            },
            "workspace": {
                "host_mount": { "path": workspace.to_string_lossy(), "readonly": false },
            },
            // Only the workspace is visible inside the sandbox, so the shell
            // agent sources its server script from there.
            "cpu_seconds": 2,
            "memory_mib": 512,
            "env": { "BASH_ENV": "/workspace/server.sh" },
            "mounts": [],
        },
        "ttl_seconds": 60,
        "idempotency_key": "k",
        "persistent": true,
    }))
}

fn worker(root: &Path, agent: PathBuf, forwards: &PortForwardRegistry) -> Result<SubprocessWorker, Box<dyn Error>> {
    Ok(SubprocessWorker::new(WorkerConfig {
        agent_binary_default: agent,
        workspace_root: root.join("workspaces"),
        isolation_backend: IsolationBackendKind::Bubblewrap,
        bwrap_binary_path: None,
        egress_proxy_binary_path: None,
        relay_binary_path: None,
        fuse: FuseConfig {
            enabled: false,
            mount_s3_binary: PathBuf::from("mount-s3"),
            region: String::new(),
            endpoint: None,
            allow_other: false,
        },
    })?
    .with_port_forwards(forwards.clone())
    .with_port_bridge_binary(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"))))
}

/// Writes the server script into a workspace directory and returns it.
fn workspace_with_server(root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let workspace = root.join("lease-workspace");
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(workspace.join("server.sh"), SERVER)?;
    Ok(workspace)
}

async fn resolve_when_listening(
    forwards: &PortForwardRegistry,
    lease: &LeaseId,
) -> Result<GetSandboxDomainResponse, Box<dyn Error>> {
    let endpoint = match forwards.resolve(lease, LEASE_PORT).await {
        GetSandboxDomainResponse::Available { endpoint, .. } => endpoint,
        other => return Ok(other),
    };
    // The server needs a moment to bind; a forward to a closed port resets.
    for _ in 0..100 {
        if let Ok(mut probe) = TcpStream::connect(&endpoint).await {
            probe.write_all(b"ready").await?;
            probe.shutdown().await?;
            let mut reply = Vec::new();
            if probe.read_to_end(&mut reply).await.is_ok() && reply == b"pong:ready" {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(forwards.resolve(lease, LEASE_PORT).await)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_in_a_bubblewrap_lease_is_reached_through_the_forward() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let root = tempfile::TempDir::new()?;
    let workspace = workspace_with_server(root.path())?;
    let agent = PathBuf::from("/bin/bash");
    let forwards = PortForwardRegistry::new();
    let worker = worker(root.path(), agent.clone(), &forwards)?;
    let lease = LeaseId::new("forward-lease");
    worker.lease(&lease_request(&agent, &workspace)?, &lease).await?;

    // The lease's loopback is its own: nothing answers on the host's.
    assert!(
        TcpStream::connect(("127.0.0.1", LEASE_PORT)).await.is_err(),
        "the lease's port is on the host loopback"
    );

    let GetSandboxDomainResponse::Available { endpoint, .. } = resolve_when_listening(&forwards, &lease).await? else {
        return Err("expected an endpoint for the lease's port".into());
    };
    let mut client = TcpStream::connect(&endpoint).await?;
    client.write_all(b"hello").await?;
    client.shutdown().await?;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut reply))
        .await
        .map_err(|_| "the reply never finished")??;
    assert_eq!(reply, b"pong:hello");

    // A port nothing listens on still gets a listener, and the client sees a
    // closed connection rather than a hang.
    let GetSandboxDomainResponse::Available { endpoint: closed, .. } = forwards.resolve(&lease, 18081).await else {
        return Err("expected a forward for an unused port".into());
    };
    let mut refused = TcpStream::connect(&closed).await?;
    let mut nothing = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), refused.read_to_end(&mut nothing))
        .await
        .map_err(|_| "a forward to a closed port left the client hanging")?
        .unwrap_or_default();
    assert!(nothing.is_empty());

    worker.cancel(&lease).await?;
    for _ in 0..50 {
        if !forwards.holds(&lease).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!forwards.holds(&lease).await, "the lease outlived its cancel");
    assert_eq!(
        forwards.resolve(&lease, LEASE_PORT).await,
        GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound {
            lease_id: lease.clone()
        })
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        TcpStream::connect(&endpoint).await.is_err(),
        "the forward outlived the lease"
    );
    Ok(())
}
