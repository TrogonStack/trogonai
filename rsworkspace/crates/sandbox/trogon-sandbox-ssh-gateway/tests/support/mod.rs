//! Shared fixtures: a scripted worker behind the tunnel trait, an SSH client, and a running gateway.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures::StreamExt;
use russh::client;
use russh::keys::PublicKey;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_auth::{ApiKeyRecord, BearerToken, InMemoryApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId, TerminalOp, TerminalResponse};
use trogon_sandbox_ssh_gateway::{
    EphemeralSessionKey, GatewayConfig, GatewayState, GuestChannel, GuestError, SshAttachRequest, SshAttachResponse,
    SshChannelKind, SshDetachRequest, SshDetachResponse, SshSessionId, SshStreamControl, SshStreamFrame,
    SshTunnelBackend, TunnelError, run_server,
};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub const API_KEY: &str = "test-bearer-token";
pub const TENANT: &str = "tenant-a";

/// A tunnel whose worker follows a script: a shell greets and echoes, `exec` commands name their own outcome.
#[derive(Default)]
pub struct FakeTunnel {
    pub attaches: Mutex<Vec<(TenantId, SshAttachRequest)>>,
    pub detaches: Mutex<Vec<(TenantId, SshDetachRequest)>>,
    pub frames: Mutex<Vec<SshStreamFrame>>,
    senders: Mutex<HashMap<SshSessionId, mpsc::Sender<SshStreamFrame>>>,
    pub refuse_attach: AtomicBool,
    pub refuse_detach: AtomicBool,
    pub refuse_subscribe: AtomicBool,
    pub refuse_publish: AtomicBool,
}

impl FakeTunnel {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn attach_count(&self) -> usize {
        self.attaches.lock().map(|attaches| attaches.len()).unwrap_or_default()
    }

    pub fn detach_count(&self) -> usize {
        self.detaches.lock().map(|detaches| detaches.len()).unwrap_or_default()
    }

    pub fn published(&self) -> Vec<SshStreamFrame> {
        self.frames.lock().map(|frames| frames.clone()).unwrap_or_default()
    }

    pub fn detach_reasons(&self) -> Vec<String> {
        self.detaches
            .lock()
            .map(|detaches| detaches.iter().map(|(_, request)| request.reason.clone()).collect())
            .unwrap_or_default()
    }

    async fn reply(&self, session_id: &SshSessionId, frame: SshStreamFrame) {
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|senders| senders.get(session_id).cloned());
        for sender in sender.into_iter() {
            let _ = sender.send(frame.clone()).await;
        }
    }

    async fn script(&self, frame: &SshStreamFrame) {
        let id = &frame.session_id;
        let channel = frame.channel_id;
        match &frame.ctl {
            Some(SshStreamControl::Open {
                kind: SshChannelKind::Shell,
                ..
            }) => {
                // A frame for a channel the client never opened is ignored.
                self.reply(id, SshStreamFrame::data(id, channel + 100, b"stray".to_vec()))
                    .await;
                self.reply(id, SshStreamFrame::data(id, channel, b"shell ready\r\n".to_vec()))
                    .await;
            }
            Some(SshStreamControl::Open {
                kind: SshChannelKind::Exec { command },
                ..
            }) => match command.as_str() {
                "boom" => {
                    let error = SshStreamControl::Error {
                        message: "no such thing".to_string(),
                    };
                    self.reply(id, SshStreamFrame::control(id, channel, error)).await;
                    self.reply(id, SshStreamFrame::eof(id, channel)).await;
                }
                "killed" => {
                    let exit = SshStreamControl::Exit { status: None };
                    self.reply(id, SshStreamFrame::control(id, channel, exit)).await;
                    self.reply(id, SshStreamFrame::eof(id, channel)).await;
                }
                "hang" => {}
                "ignored-controls" => {
                    for one in trogon_sandbox_contract::TerminalDimension::new(1).into_iter() {
                        let open = SshStreamControl::Resize { cols: one, rows: one };
                        self.reply(id, SshStreamFrame::control(id, channel, open)).await;
                    }
                    self.reply(id, SshStreamFrame::data(id, channel, Vec::new())).await;
                    let exit = SshStreamControl::Exit { status: Some(0) };
                    self.reply(id, SshStreamFrame::control(id, channel, exit)).await;
                    self.reply(id, SshStreamFrame::eof(id, channel)).await;
                }
                other => {
                    self.reply(
                        id,
                        SshStreamFrame::data(id, channel, format!("ran: {other}").into_bytes()),
                    )
                    .await;
                    let exit = SshStreamControl::Exit { status: Some(3) };
                    self.reply(id, SshStreamFrame::control(id, channel, exit)).await;
                    self.reply(id, SshStreamFrame::eof(id, channel)).await;
                }
            },
            Some(_) => {}
            None if frame.eof => {
                let exit = SshStreamControl::Exit { status: Some(0) };
                self.reply(id, SshStreamFrame::control(id, channel, exit)).await;
                self.reply(id, SshStreamFrame::eof(id, channel)).await;
            }
            None => {
                self.reply(id, SshStreamFrame::data(id, channel, frame.data.clone()))
                    .await;
            }
        }
    }
}

#[async_trait]
impl SshTunnelBackend for FakeTunnel {
    async fn attach(&self, tenant: &TenantId, request: &SshAttachRequest) -> Result<SshAttachResponse, TunnelError> {
        if self.refuse_attach.load(Ordering::SeqCst) {
            return Err(TunnelError::AttachRejected("no worker".to_string()));
        }
        if let Ok(mut attaches) = self.attaches.lock() {
            attaches.push((tenant.clone(), request.clone()));
        }
        Ok(SshAttachResponse::accepted())
    }

    async fn detach(&self, tenant: &TenantId, request: &SshDetachRequest) -> Result<SshDetachResponse, TunnelError> {
        if let Ok(mut detaches) = self.detaches.lock() {
            detaches.push((tenant.clone(), request.clone()));
        }
        if self.refuse_detach.load(Ordering::SeqCst) {
            return Err(TunnelError::DetachRejected("gone".to_string()));
        }
        Ok(SshDetachResponse::accepted())
    }

    async fn publish_frame(&self, frame: &SshStreamFrame) -> Result<(), TunnelError> {
        if self.refuse_publish.load(Ordering::SeqCst) {
            return Err(TunnelError::Timeout(Duration::from_secs(1)));
        }
        if let Ok(mut frames) = self.frames.lock() {
            frames.push(frame.clone());
        }
        self.script(frame).await;
        Ok(())
    }

    async fn subscribe_frames(&self, session_id: &SshSessionId) -> Result<mpsc::Receiver<SshStreamFrame>, TunnelError> {
        if self.refuse_subscribe.load(Ordering::SeqCst) {
            return Err(TunnelError::Timeout(Duration::from_secs(1)));
        }
        let (sender, receiver) = mpsc::channel(64);
        if let Ok(mut senders) = self.senders.lock() {
            senders.insert(session_id.clone(), sender);
        }
        Ok(receiver)
    }
}

pub async fn api_keys() -> TestResult<Arc<InMemoryApiKeyStore>> {
    let store = Arc::new(InMemoryApiKeyStore::new());
    let record = ApiKeyRecord::new(
        TenantId::new(TENANT)?,
        ServiceAccountId::new("sa-test"),
        "test",
        SystemTime::now(),
    );
    store.insert(BearerToken::new(API_KEY)?.hash(), record).await;
    Ok(store)
}

/// A running gateway and what a test needs to talk to and stop it.
pub struct Gateway {
    pub addr: SocketAddr,
    pub tunnel: Arc<FakeTunnel>,
    pub cancel: CancellationToken,
    pub server: JoinHandle<Result<(), trogon_sandbox_ssh_gateway::ServeError>>,
}

pub async fn start_gateway(max_connections: usize) -> TestResult<Gateway> {
    start_gateway_with(FakeTunnel::new(), max_connections, None).await
}

pub async fn start_gateway_with(
    tunnel: Arc<FakeTunnel>,
    max_connections: usize,
    key_mint: Option<Arc<trogon_sandbox_ssh_gateway::SessionKeyMint>>,
) -> TestResult<Gateway> {
    let state = Arc::new(GatewayState::new(GatewayConfig {
        api_keys: api_keys().await?,
        tunnel: Arc::clone(&tunnel) as Arc<dyn SshTunnelBackend>,
        key_mint,
        max_connections: std::num::NonZeroUsize::new(max_connections).ok_or("zero connections")?,
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let host_key = EphemeralSessionKey::generate()?.key_pair;
    let server = tokio::spawn(run_server(listener, host_key, state, cancel.clone()));
    Ok(Gateway {
        addr,
        tunnel,
        cancel,
        server,
    })
}

pub struct AcceptAnyHostKey;

impl client::Handler for AcceptAnyHostKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub async fn connect(addr: SocketAddr) -> TestResult<client::Handle<AcceptAnyHostKey>> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    Ok(client::connect(config, addr, AcceptAnyHostKey).await?)
}

/// Connects and authenticates as `lease@gw` with the valid API key.
pub async fn login(addr: SocketAddr, lease: &str) -> TestResult<client::Handle<AcceptAnyHostKey>> {
    let mut session = connect(addr).await?;
    let auth = session.authenticate_password(format!("{lease}@gw"), API_KEY).await?;
    assert!(auth.success(), "password authentication failed");
    Ok(session)
}

/// Waits until `check` holds, for effects that happen on another task.
pub async fn eventually(mut check: impl FnMut() -> bool) -> TestResult {
    let wait = async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    Ok(tokio::time::timeout(Duration::from_secs(5), wait).await?)
}

/// A NATS server running in a container, stopped when dropped.
pub struct Server {
    inner: trogon_nats::test_support::CoreTestServer,
}

impl Server {
    pub async fn start() -> Self {
        Self {
            inner: trogon_nats::test_support::CoreTestServer::start().await,
        }
    }

    pub async fn start_with_config(config: &str) -> Self {
        Self {
            inner: trogon_nats::test_support::CoreTestServer::start_with_config(config).await,
        }
    }

    pub fn url(&self) -> String {
        format!("nats://{}", self.inner.address())
    }

    pub async fn client(&self) -> Result<async_nats::Client, async_nats::ConnectError> {
        async_nats::connect(self.url()).await
    }
}

pub async fn start_nats() -> TestResult<(Server, async_nats::Client)> {
    let server = Server::start().await;
    let client = server.client().await?;
    Ok((server, client))
}

/// Answers every request on `subject` with whatever `answer` returns (nothing leaves the request unanswered), and
/// hands each request to the test.
pub async fn respond(
    client: &async_nats::Client,
    subject: &str,
    answer: impl Fn(&async_nats::Message) -> Option<bytes::Bytes> + Send + Sync + 'static,
) -> TestResult<mpsc::UnboundedReceiver<async_nats::Message>> {
    let subscription = client.subscribe(subject.to_string()).await?;
    client.flush().await?;
    let (seen_tx, seen_rx) = mpsc::unbounded_channel();
    let client = client.clone();
    let answer = Arc::new(answer);
    tokio::spawn(subscription.for_each(move |message| {
        let client = client.clone();
        let seen_tx = seen_tx.clone();
        let answer = Arc::clone(&answer);
        async move {
            if let (Some(reply), Some(body)) = (message.reply.clone(), answer(&message)) {
                let _ = client.publish(reply, body).await;
            }
            let _ = seen_tx.send(message);
        }
    }));
    Ok(seen_rx)
}

pub fn json(value: &impl serde::Serialize) -> TestResult<bytes::Bytes> {
    Ok(bytes::Bytes::from(serde_json::to_vec(value)?))
}

/// A worker that holds some leases and answers terminal requests from a script.
pub struct FakeHost {
    leases: Vec<trogon_sandbox_contract::LeaseId>,
    pub requests: Mutex<Vec<trogon_sandbox_contract::TerminalRequest>>,
    pub refuse_open: AtomicBool,
    pub refuse_ops: AtomicBool,
}

impl FakeHost {
    pub fn holding(leases: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            leases: leases
                .iter()
                .map(|lease| trogon_sandbox_contract::LeaseId::new(*lease))
                .collect(),
            requests: Mutex::new(Vec::new()),
            refuse_open: AtomicBool::new(false),
            refuse_ops: AtomicBool::new(false),
        })
    }

    pub fn ops(&self) -> Vec<trogon_sandbox_contract::TerminalOp> {
        self.requests
            .lock()
            .map(|requests| requests.iter().map(|request| request.op.clone()).collect())
            .unwrap_or_default()
    }

    pub fn first_terminal(&self) -> Option<trogon_sandbox_contract::TerminalId> {
        self.requests
            .lock()
            .ok()
            .and_then(|requests| requests.first().map(|request| request.terminal_id.clone()))
    }
}

#[async_trait]
impl trogon_sandbox_ssh_gateway::SshTerminalHost for FakeHost {
    async fn holds(&self, lease_id: &trogon_sandbox_contract::LeaseId) -> bool {
        self.leases.contains(lease_id)
    }

    async fn terminal(
        &self,
        request: &trogon_sandbox_contract::TerminalRequest,
    ) -> trogon_sandbox_contract::TerminalResponse {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(request.clone());
        }
        match &request.op {
            TerminalOp::Open { .. } if self.refuse_open.load(Ordering::SeqCst) => TerminalResponse::Unsupported,
            TerminalOp::Open { .. } => TerminalResponse::Opened {
                terminal_id: request.terminal_id.clone(),
            },
            _ if self.refuse_ops.load(Ordering::SeqCst) => TerminalResponse::Unsupported,
            _ => TerminalResponse::Done,
        }
    }
}

pub async fn next_frame(subscription: &mut async_nats::Subscriber) -> TestResult<SshStreamFrame> {
    let message = tokio::time::timeout(Duration::from_secs(10), subscription.next())
        .await?
        .ok_or("subscription ended")?;
    Ok(serde_json::from_slice(&message.payload)?)
}

/// The test's side of one terminal opened through [`FakeGuest`].
pub struct GuestEnd {
    /// What the hub wrote to the terminal's input.
    pub stdin: tokio::io::DuplexStream,
    /// Write here to print on the terminal.
    pub stdout: tokio::io::DuplexStream,
    pub resize: mpsc::Receiver<(
        trogon_sandbox_contract::TerminalDimension,
        trogon_sandbox_contract::TerminalDimension,
    )>,
    pub done: tokio::sync::oneshot::Sender<i32>,
    pub close: CancellationToken,
}

/// A guest that holds some leases and hands each opened terminal to the test.
pub type OpenedShell = (String, Option<String>, (u16, u16));

pub struct FakeGuest {
    leases: Vec<trogon_sandbox_contract::LeaseId>,
    pub opened: Mutex<Vec<OpenedShell>>,
    pub released: Mutex<Vec<trogon_sandbox_contract::LeaseId>>,
    pub ends: tokio::sync::Mutex<Vec<GuestEnd>>,
    pub open_error: Mutex<Option<String>>,
    pub stdin_capacity: usize,
}

impl FakeGuest {
    pub fn holding(leases: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            leases: leases
                .iter()
                .map(|lease| trogon_sandbox_contract::LeaseId::new(*lease))
                .collect(),
            opened: Mutex::new(Vec::new()),
            released: Mutex::new(Vec::new()),
            ends: tokio::sync::Mutex::new(Vec::new()),
            open_error: Mutex::new(None),
            stdin_capacity: 64 * 1024,
        })
    }

    pub fn with_stdin_capacity(leases: &[&str], stdin_capacity: usize) -> Arc<Self> {
        let mut guest = Self::holding(leases);
        if let Some(inner) = Arc::get_mut(&mut guest) {
            inner.stdin_capacity = stdin_capacity;
        }
        guest
    }

    pub async fn take_end(&self) -> TestResult<GuestEnd> {
        eventually_async(|| async { !self.ends.lock().await.is_empty() }).await?;
        Ok(self.ends.lock().await.remove(0))
    }
}

pub async fn eventually_async<F, Fut>(mut check: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    // The first tick completes at once, so every line here runs even when the check passes first time.
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let wait = async {
        loop {
            tick.tick().await;
            if check().await {
                break;
            }
        }
    };
    Ok(tokio::time::timeout(Duration::from_secs(5), wait).await?)
}

#[async_trait]
impl trogon_sandbox_ssh_gateway::GuestShell for FakeGuest {
    async fn holds(&self, lease_id: &trogon_sandbox_contract::LeaseId) -> bool {
        self.leases.contains(lease_id)
    }

    async fn open(
        &self,
        _lease_id: &trogon_sandbox_contract::LeaseId,
        size: (
            trogon_sandbox_contract::TerminalDimension,
            trogon_sandbox_contract::TerminalDimension,
        ),
        term: &str,
        command: Option<String>,
    ) -> Result<trogon_sandbox_ssh_gateway::GuestChannel, trogon_sandbox_ssh_gateway::GuestError> {
        let refusal = self.open_error.lock().ok().and_then(|error| error.clone());
        if let Some(reason) = refusal {
            return Err(GuestError::Unavailable(reason.into()));
        }
        if let Ok(mut opened) = self.opened.lock() {
            opened.push((term.to_string(), command, (size.0.get(), size.1.get())));
        }
        let (stdin, stdin_rx) = tokio::io::duplex(self.stdin_capacity);
        let (stdout_tx, stdout) = tokio::io::duplex(64 * 1024);
        let (resize, resize_rx) = mpsc::channel(8);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let close = CancellationToken::new();
        self.ends.lock().await.push(GuestEnd {
            stdin: stdin_rx,
            stdout: stdout_tx,
            resize: resize_rx,
            done: done_tx,
            close: close.clone(),
        });
        Ok(GuestChannel {
            stdin: Box::pin(stdin),
            stdout: Box::pin(stdout),
            resize,
            completion: Box::pin(async move { done_rx.await.map_err(|_| GuestError::NoExitStatus) }),
            close,
        })
    }

    async fn release(&self, lease_id: &trogon_sandbox_contract::LeaseId) {
        if let Ok(mut released) = self.released.lock() {
            released.push(lease_id.clone());
        }
    }
}
