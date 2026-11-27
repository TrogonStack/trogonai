//! Shared fixtures: an in-process SSH server standing in for Daytona's SSH gateway, and a scripted
//! [`GatewayAccessSource`] that mints tokens for it.

#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use russh::keys::PrivateKey;
use russh::server::{Auth, Config, Handler, Msg, Session};
use russh::{Channel, ChannelId, Disconnect};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};
use trogon_sandbox_daytona_worker::{GatewayAccess, GatewayAccessError, GatewayAccessSource};
use trogon_sandbox_ssh_gateway::EphemeralSessionKey;

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult<T = ()> = Result<T, Boxed>;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A fresh host key, built at runtime.
pub fn host_key() -> TestResult<PrivateKey> {
    Ok(EphemeralSessionKey::generate()?.key_pair)
}

/// What the fake gateway saw.
#[derive(Default)]
pub struct Seen {
    /// Tokens the gateway accepts as a user name.
    pub valid_tokens: Vec<String>,
    /// Every user name presented, accepted or not.
    pub presented: Vec<String>,
    pub ptys: Vec<(String, u32, u32)>,
    pub execs: Vec<String>,
    pub sizes: Vec<(u32, u32)>,
    pub closed_channels: usize,
    pub logins: usize,
}

/// How the fake gateway misbehaves.
#[derive(Default)]
pub struct Faults {
    pub refuse_channels: AtomicBool,
}

/// An in-process SSH server standing in for Daytona's SSH gateway. A user name is a token and logs in with the `none`
/// method when the server was told to accept it. A shell greets with `ready`, `echo X` answers `X`, `exit` exits 3,
/// and a resize answers with the new size.
pub struct GatewayServer {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Seen>>,
    pub faults: Arc<Faults>,
    sessions: Arc<Mutex<Vec<russh::server::Handle>>>,
    accept: JoinHandle<()>,
}

impl GatewayServer {
    pub async fn start() -> TestResult<Self> {
        Self::start_seeing(Arc::default()).await
    }

    /// Starts a server that shares `seen` (and so its accepted tokens) with another.
    pub async fn start_seeing(seen: Arc<Mutex<Seen>>) -> TestResult<Self> {
        let config = Arc::new(Config {
            keys: vec![host_key()?],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let faults = Arc::new(Faults::default());
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let accept = tokio::spawn(accept_loop(
            listener,
            config,
            Arc::clone(&seen),
            Arc::clone(&faults),
            Arc::clone(&sessions),
        ));
        Ok(Self {
            addr,
            seen,
            faults,
            sessions,
            accept,
        })
    }

    pub fn seen(&self) -> MutexGuard<'_, Seen> {
        lock(&self.seen)
    }

    /// Accepts `token` as a user name from now on.
    pub fn accept_token(&self, token: &str) {
        self.seen().valid_tokens.push(token.to_owned());
    }

    /// Disconnects every client, as a gateway restart would.
    pub async fn drop_sessions(&self) {
        let sessions: Vec<_> = lock(&self.sessions).drain(..).collect();
        for session in sessions {
            let _ = session
                .disconnect(Disconnect::ByApplication, "restarting".to_string(), "en".to_string())
                .await;
        }
    }
}

impl Drop for GatewayServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn accept_loop(
    listener: TcpListener,
    config: Arc<Config>,
    seen: Arc<Mutex<Seen>>,
    faults: Arc<Faults>,
    sessions: Arc<Mutex<Vec<russh::server::Handle>>>,
) {
    while let Ok((stream, _)) = listener.accept().await {
        let handler = GatewayHandler {
            seen: Arc::clone(&seen),
            faults: Arc::clone(&faults),
            line: Vec::new(),
        };
        let (config, sessions) = (Arc::clone(&config), Arc::clone(&sessions));
        tokio::spawn(async move {
            if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                lock(&sessions).push(session.handle());
                let _ = session.await;
            }
        });
    }
}

struct GatewayHandler {
    seen: Arc<Mutex<Seen>>,
    faults: Arc<Faults>,
    line: Vec<u8>,
}

impl Handler for GatewayHandler {
    type Error = russh::Error;

    async fn auth_none(&mut self, user: &str) -> Result<Auth, Self::Error> {
        let mut seen = lock(&self.seen);
        seen.presented.push(user.to_owned());
        if seen.valid_tokens.iter().any(|token| token == user) {
            seen.logins += 1;
            return Ok(Auth::Accept);
        }
        Ok(Auth::reject())
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(!self.faults.refuse_channels.load(Ordering::SeqCst))
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        lock(&self.seen).ptys.push((term.to_owned(), col_width, row_height));
        session.channel_success(channel)
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, &b"ready\r\n"[..])
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        lock(&self.seen).execs.push(String::from_utf8_lossy(data).into_owned());
        session.channel_success(channel)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        lock(&self.seen).sizes.push((col_width, row_height));
        session.data(channel, format!("size {col_width}x{row_height}\r\n").into_bytes())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        self.line.extend_from_slice(data);
        while let Some(end) = self.line.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.line.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_owned();
            if let Some(rest) = line.strip_prefix("echo ") {
                session.data(channel, format!("{rest}\r\n").into_bytes())?;
            } else if line == "exit" {
                session.exit_status_request(channel, 3)?;
                session.close(channel)?;
            }
        }
        Ok(())
    }

    async fn channel_close(&mut self, _channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        lock(&self.seen).closed_channels += 1;
        Ok(())
    }
}

/// Mints tokens for a [`GatewayServer`]. It holds `lease-1`. The first `stale` tokens it hands out are ones the
/// gateway does not accept, to exercise the refresh.
pub struct FakeAccess {
    addr: Mutex<SocketAddr>,
    seen: Arc<Mutex<Seen>>,
    stale: usize,
    expires_at: Mutex<Option<u64>>,
    pub minted: AtomicUsize,
    /// Minting fails.
    pub fail_mint: AtomicBool,
}

impl FakeAccess {
    pub fn new(server: &GatewayServer) -> Arc<Self> {
        Self::stale(server, 0)
    }

    /// An access whose first `stale` tokens the gateway refuses.
    pub fn stale(server: &GatewayServer, stale: usize) -> Arc<Self> {
        Self::at(server.addr, Arc::clone(&server.seen), stale)
    }

    pub fn at(addr: SocketAddr, seen: Arc<Mutex<Seen>>, stale: usize) -> Arc<Self> {
        Arc::new(Self {
            addr: Mutex::new(addr),
            seen,
            stale,
            expires_at: Mutex::new(None),
            minted: AtomicUsize::new(0),
            fail_mint: AtomicBool::new(false),
        })
    }

    pub fn point_at(&self, addr: SocketAddr) {
        *lock(&self.addr) = addr;
    }

    /// Reports every token minted from now on as expiring at `at` unix seconds.
    pub fn expire_at(&self, at: Option<u64>) {
        *lock(&self.expires_at) = at;
    }

    pub fn mint_count(&self) -> usize {
        self.minted.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl GatewayAccessSource for FakeAccess {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        lease_id.as_str() == "lease-1"
    }

    async fn mint(&self, lease_id: &LeaseId) -> Result<GatewayAccess, GatewayAccessError> {
        if self.fail_mint.load(Ordering::SeqCst) {
            return Err(GatewayAccessError::UnknownLease(lease_id.clone()));
        }
        let number = self.minted.fetch_add(1, Ordering::SeqCst) + 1;
        let token = format!("token-{number}");
        if number > self.stale {
            lock(&self.seen).valid_tokens.push(token.clone());
        }
        let addr = *lock(&self.addr);
        Ok(GatewayAccess {
            host: addr.ip().to_string(),
            port: addr.port(),
            token,
            expires_at: *lock(&self.expires_at),
        })
    }
}

/// A `(cols, rows)` terminal size.
pub fn size(cols: u16, rows: u16) -> TestResult<(TerminalDimension, TerminalDimension)> {
    Ok((TerminalDimension::new(cols)?, TerminalDimension::new(rows)?))
}

/// Reads a terminal's output until it contains `want`, failing after ten seconds or at the end of the output.
pub async fn read_until(stdout: &mut Pin<Box<dyn AsyncRead + Send>>, want: &str) -> TestResult<String> {
    let read = async {
        let mut seen = String::new();
        let mut buf = [0_u8; 1024];
        while !seen.contains(want) {
            let read = stdout.read(&mut buf).await?;
            let bytes = buf.get(..read).filter(|bytes| !bytes.is_empty());
            let bytes = bytes.ok_or_else(|| format!("the output ended before {want:?}; saw {seen:?}"))?;
            seen.push_str(&String::from_utf8_lossy(bytes));
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(seen)
    };
    tokio::time::timeout(Duration::from_secs(10), read).await?
}

/// Polls `check` until it holds, failing after thirty seconds.
pub async fn eventually(mut check: impl FnMut() -> bool) -> TestResult {
    let wait = async {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        while !check() {
            tick.tick().await;
        }
    };
    Ok(tokio::time::timeout(Duration::from_secs(30), wait).await?)
}

/// Sends `body` to `subject` until something answers, for handlers that subscribe in a spawned task.
///
/// A request that finds no subscriber yet is sent again. One that was delivered is waited for, never re-sent.
pub async fn request_until_answered(
    client: &async_nats::Client,
    subject: &str,
    body: Vec<u8>,
) -> TestResult<async_nats::Message> {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    for _ in 0..300 {
        tick.tick().await;
        let reply = tokio::time::timeout(
            Duration::from_secs(30),
            client.request(subject.to_string(), body.clone().into()),
        )
        .await?;
        if let Ok(message) = reply {
            return Ok(message);
        }
    }
    Err(format!("no subscriber on {subject}").into())
}

/// Whether a request to `subject` goes unanswered for a short while.
pub async fn unanswered(client: &async_nats::Client, subject: &str, body: Vec<u8>) -> bool {
    let reply = tokio::time::timeout(
        Duration::from_millis(700),
        client.request(subject.to_string(), body.into()),
    )
    .await;
    !matches!(reply, Ok(Ok(_)))
}
