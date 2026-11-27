#![cfg(any(target_os = "linux", target_os = "macos"))]

//! The gateway's side of the SSH stream protocol, driven over a real NATS
//! server against the subprocess worker's bridge: exec and shell sessions end
//! to end, and a lease the worker does not hold is refused.

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use russh::keys::PublicKey;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyRecord, BearerToken, InMemoryApiKeyStore};
use trogon_sandbox_contract::{LeaseId, ServiceAccountId, TenantId, TerminalDimension};
use trogon_sandbox_ssh_gateway::{
    BridgeLimits, EphemeralSessionKey, GatewayConfig, GatewayState, NatsSshTunnelBackend, SshAttachRequest,
    SshChannelKind, SshDetachRequest, SshSessionId, SshStreamBridge, SshStreamControl, SshStreamFrame,
    SshTunnelBackend, run_server,
};
use trogon_sandbox_subprocess_worker::isolation::IsolationBackend;
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::ssh_bridge::SubprocessSshHost;
use trogon_sandbox_subprocess_worker::terminal::{FrameSink, SubprocessTerminals};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};
use trogon_std::env::SystemEnv;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const TENANT: &str = "tenant-a";
const LEASE: &str = "lease-a";
const READY_WINDOW: Duration = Duration::from_secs(30);

#[cfg(target_os = "macos")]
fn backend() -> Option<Arc<dyn IsolationBackend>> {
    Some(Arc::new(
        trogon_sandbox_subprocess_worker::isolation::SeatbeltBackend::new(),
    ))
}

#[cfg(target_os = "linux")]
fn backend() -> Option<Arc<dyn IsolationBackend>> {
    let bwrap = which::which("bwrap").ok()?;
    Some(Arc::new(
        trogon_sandbox_subprocess_worker::isolation::BubblewrapBackend::new(bwrap),
    ))
}

fn spec_in(root: &Path) -> LaunchSpec {
    let root = root.to_path_buf();
    LaunchSpec {
        binary: PathBuf::from("/bin/sleep"),
        args: vec!["60".to_string()],
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        workspace_host_path: root.clone(),
        workspace_sandbox_path: if cfg!(target_os = "linux") {
            PathBuf::from("/workspace")
        } else {
            root.clone()
        },
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    }
}

struct Rig {
    client: async_nats::Client,
    _server: JetStreamTestServer,
    tunnel: NatsSshTunnelBackend,
    tenant: TenantId,
    cancel: CancellationToken,
    _workspace: tempfile::TempDir,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn rig(backend: Arc<dyn IsolationBackend>) -> TestResult<Rig> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let workspace = tempfile::tempdir()?;
    let root = workspace.path().canonicalize()?;
    let terminals = SubprocessTerminals::new(Arc::new(client.clone()) as Arc<dyn FrameSink>, &SystemEnv);
    terminals.register_lease(&LeaseId::new(LEASE), spec_in(&root), backend);
    let bridge = SshStreamBridge::new(
        Arc::new(SubprocessSshHost(terminals)),
        client.clone(),
        BridgeLimits::default(),
    );
    let cancel = CancellationToken::new();
    tokio::spawn(bridge.serve(client.clone(), cancel.clone()));
    let rig = Rig {
        client: client.clone(),
        _server: server,
        tunnel: NatsSshTunnelBackend::new(client).with_timeout(Duration::from_secs(5)),
        tenant: TenantId::new(TENANT)?,
        cancel,
        _workspace: workspace,
    };
    rig.wait_until_serving().await?;
    Ok(rig)
}

impl Rig {
    /// Attaches and detaches a probe session until the bridge answers, which
    /// means its attach and detach subscriptions have reached the server.
    async fn wait_until_serving(&self) -> TestResult {
        let session_id = SshSessionId::new("ready-probe")?;
        let deadline = tokio::time::Instant::now() + READY_WINDOW;
        loop {
            let attached = self
                .tunnel
                .attach(&self.tenant, &attach_request(&session_id, LEASE)?)
                .await;
            if attached.is_ok() {
                let detach = detach_request(&session_id, LEASE);
                self.tunnel.detach(&self.tenant, &detach).await?;
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("the bridge never answered an attach".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn attach_request(session_id: &SshSessionId, lease: &str) -> TestResult<SshAttachRequest> {
    Ok(SshAttachRequest {
        session_id: session_id.clone(),
        lease_id: LeaseId::new(lease),
        public_key: "ssh-ed25519 AAAA test".to_string(),
        ttl_seconds: std::num::NonZeroU32::new(120).ok_or("zero ttl")?,
    })
}

fn detach_request(session_id: &SshSessionId, lease: &str) -> SshDetachRequest {
    SshDetachRequest {
        session_id: session_id.clone(),
        lease_id: LeaseId::new(lease),
        reason: "test".to_string(),
    }
}

/// Reads frames for `channel_id` until its eof, returning the text and the
/// exit status the worker reported.
async fn drain(rx: &mut Receiver<SshStreamFrame>, channel_id: u32) -> TestResult<(String, Option<u32>)> {
    let mut bytes = Vec::new();
    let mut status = None;
    let mut failure = None;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(frame) = rx.recv().await {
            if frame.channel_id != channel_id {
                continue;
            }
            match frame.ctl {
                Some(SshStreamControl::Exit { status: s }) => status = s,
                Some(SshStreamControl::Error { message }) => {
                    failure = Some(message);
                    return;
                }
                _ => bytes.extend(frame.data),
            }
            if frame.eof {
                return;
            }
        }
    })
    .await;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if let Some(message) = failure {
        return Err(format!("worker error: {message}").into());
    }
    assert!(result.is_ok(), "timed out; output so far: {text:?}");
    Ok((text, status))
}

fn open(session_id: &SshSessionId, channel_id: u32, kind: SshChannelKind) -> TestResult<SshStreamFrame> {
    Ok(SshStreamFrame::control(
        session_id,
        channel_id,
        SshStreamControl::Open {
            kind,
            cols: TerminalDimension::new(80)?,
            rows: TerminalDimension::new(24)?,
            term: Some("xterm".to_string()),
        },
    ))
}

#[tokio::test]
async fn an_exec_session_returns_the_output_and_exit_status() -> TestResult {
    let Some(backend) = backend() else {
        return Ok(());
    };
    let rig = rig(backend).await?;
    let session_id = SshSessionId::new("sess-exec")?;
    rig.tunnel
        .attach(&rig.tenant, &attach_request(&session_id, LEASE)?)
        .await?;
    let mut rx = rig.tunnel.subscribe_frames(&session_id).await?;

    let command = SshChannelKind::Exec {
        command: "echo hi".to_string(),
    };
    rig.tunnel.publish_frame(&open(&session_id, 0, command)?).await?;
    let (text, status) = drain(&mut rx, 0).await?;
    assert!(text.contains("hi"), "output: {text:?}");
    assert_eq!(status, Some(0));

    let failing = SshChannelKind::Exec {
        command: "exit 7".to_string(),
    };
    rig.tunnel.publish_frame(&open(&session_id, 1, failing)?).await?;
    let (_, status) = drain(&mut rx, 1).await?;
    assert_eq!(status, Some(7));

    rig.tunnel
        .detach(&rig.tenant, &detach_request(&session_id, LEASE))
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_shell_session_runs_what_the_client_types() -> TestResult {
    let Some(backend) = backend() else {
        return Ok(());
    };
    let rig = rig(backend).await?;
    let session_id = SshSessionId::new("sess-shell")?;
    rig.tunnel
        .attach(&rig.tenant, &attach_request(&session_id, LEASE)?)
        .await?;
    let mut rx = rig.tunnel.subscribe_frames(&session_id).await?;

    rig.tunnel
        .publish_frame(&open(&session_id, 0, SshChannelKind::Shell)?)
        .await?;
    rig.tunnel
        .publish_frame(&SshStreamFrame::data(&session_id, 0, b"echo hi\nexit\n".to_vec()))
        .await?;
    let (text, status) = drain(&mut rx, 0).await?;
    assert!(text.contains("hi"), "output: {text:?}");
    assert_eq!(status, Some(0));
    Ok(())
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_is_not_attached() -> TestResult {
    let Some(backend) = backend() else {
        return Ok(());
    };
    let rig = rig(backend).await?;
    let session_id = SshSessionId::new("sess-other")?;
    // No worker holds this lease, so nothing answers the attach.
    let refused = rig
        .tunnel
        .attach(&rig.tenant, &attach_request(&session_id, "lease-not-mine")?)
        .await;
    assert!(refused.is_err(), "unexpected attach: {refused:?}");

    // Frames for a session that never attached reach no terminal.
    let mut rx = rig.tunnel.subscribe_frames(&session_id).await?;
    let command = SshChannelKind::Exec {
        command: "echo hi".to_string(),
    };
    rig.tunnel.publish_frame(&open(&session_id, 0, command)?).await?;
    let silent = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(silent.is_err(), "worker answered an unattached session");
    Ok(())
}

struct AcceptAnyKey;

impl russh::client::Handler for AcceptAnyKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Reads a channel until it closes, returning its output and exit status.
async fn read_channel(channel: &mut russh::Channel<russh::client::Msg>) -> (String, Option<u32>) {
    let mut bytes = Vec::new();
    let mut status = None;
    let done = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(msg) = channel.wait().await {
            match msg {
                russh::ChannelMsg::Data { data } => bytes.extend_from_slice(&data),
                russh::ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                russh::ChannelMsg::Close => return,
                _ => {}
            }
        }
    })
    .await;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(done.is_ok(), "timed out; output so far: {text:?}");
    (text, status)
}

async fn api_keys(bearer: &str) -> TestResult<Arc<InMemoryApiKeyStore>> {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let record = ApiKeyRecord::new(
        TenantId::new(TENANT)?,
        ServiceAccountId::new("sa-test"),
        "test",
        SystemTime::now(),
    );
    store.insert(BearerToken::new(bearer)?.hash(), record).await;
    Ok(store)
}

async fn login(
    config: &Arc<russh::client::Config>,
    addr: SocketAddr,
    bearer: &str,
) -> TestResult<russh::client::Handle<AcceptAnyKey>> {
    let mut session = russh::client::connect(Arc::clone(config), addr, AcceptAnyKey).await?;
    let auth = session.authenticate_password(format!("{LEASE}@gw"), bearer).await?;
    assert!(auth.success());
    Ok(session)
}

#[tokio::test]
async fn an_ssh_client_gets_a_shell_and_exec_through_the_gateway() -> TestResult {
    let Some(backend) = backend() else {
        return Ok(());
    };
    let rig = rig(backend).await?;
    let bearer = format!("bearer-{}", uuid::Uuid::now_v7().simple());
    let state = Arc::new(GatewayState::new(GatewayConfig {
        api_keys: api_keys(&bearer).await?,
        tunnel: Arc::new(NatsSshTunnelBackend::new(rig.client.clone())),
        key_mint: None,
        max_connections: std::num::NonZeroUsize::new(4).ok_or("zero connections")?,
    }));
    let host_key = EphemeralSessionKey::generate()?.key_pair;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(run_server(listener, host_key, state, rig.cancel.clone()));

    // An exec request, as `ssh lease@gw echo hi` sends.
    let config = Arc::new(russh::client::Config::default());
    let session = login(&config, addr, &bearer).await?;
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, "echo hi").await?;
    let (text, status) = read_channel(&mut channel).await;
    assert!(text.contains("hi"), "exec output: {text:?}");
    assert_eq!(status, Some(0));
    drop(session);

    // A pty and a shell, as a bare `ssh -t lease@gw` sends.
    let session = login(&config, addr, &bearer).await?;
    let mut channel = session.channel_open_session().await?;
    channel.request_pty(true, "xterm", 100, 30, 0, 0, &[]).await?;
    channel.request_shell(true).await?;
    channel.data(&b"stty size; exit 3\n"[..]).await?;
    let (text, status) = read_channel(&mut channel).await;
    assert!(text.contains("30 100"), "shell output: {text:?}");
    assert_eq!(status, Some(3));
    Ok(())
}
