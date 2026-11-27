//! Shared fixtures: fake `virtctl`/`kubectl` scripts, a scripted lease locator, an in-process SSH server standing in
//! for a guest's `sshd`, a scripted [`GuestAccess`], and a NATS server.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use russh::keys::{PrivateKey, PublicKey, PublicKeyBase64};
use russh::server::{Auth, Config, Handler, Msg, Session};
use russh::{Channel, ChannelId, Disconnect};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};
use trogon_sandbox_kubevirt_worker::{GUEST_USER, GuestAccess, GuestAccessError, GuestLocator};
use trogon_sandbox_ssh_gateway::EphemeralSessionKey;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Serializes the tests that write and run scripts. A script written while another test forks can be held open for
/// writing by the forked child, and running it then fails with `ETXTBSY` on Linux.
pub static SCRIPTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Writes an executable `/bin/sh` script named `name` into `dir`.
pub fn script(dir: &Path, name: &str, body: &str) -> TestResult<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

/// Ends each invocation a [`logging`] script records.
const END: &str = "<<end>>";

/// How many times [`Nats::client`] tries to connect before giving up.
const CONNECT_ATTEMPTS: u32 = 50;
/// Longest a delivered request may wait for its handler, which a loaded machine can keep busy for a while.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

/// A script body that appends each invocation's arguments to `log`, one per line, then an end marker.
pub fn logging(log: &Path) -> String {
    format!("printf '%s\\n' \"$@\" '{END}' >> '{}'", log.display())
}

/// The invocations a [`logging`] script recorded, each as its arguments.
pub fn invocations(log: &Path) -> TestResult<Vec<Vec<String>>> {
    let text = match std::fs::read_to_string(log) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut calls = Vec::new();
    let mut current = Vec::new();
    for line in text.lines() {
        if line == END {
            calls.push(std::mem::take(&mut current));
        } else {
            current.push(line.to_string());
        }
    }
    Ok(calls)
}

/// A fresh `authorized_keys` line, built at runtime.
pub fn public_key_line() -> TestResult<String> {
    Ok(EphemeralSessionKey::generate()?.public_key_line)
}

/// A fresh host key, built at runtime.
pub fn host_key() -> TestResult<PrivateKey> {
    Ok(EphemeralSessionKey::generate()?.key_pair)
}

/// The command a `bash -lc` invocation ran: its last argument.
pub fn command_of(call: &[String]) -> Option<&str> {
    call.last().map(String::as_str)
}

/// A lease locator with scripted answers.
#[derive(Default)]
pub struct FakeLocator {
    vms: Mutex<HashMap<LeaseId, String>>,
    ips: Mutex<HashMap<String, String>>,
    /// When set, how many more `vm_name` lookups find their lease; later ones find nothing.
    lookups_left: Mutex<Option<usize>>,
}

impl FakeLocator {
    pub fn holding(lease: &str, vm_name: &str) -> Arc<Self> {
        let locator = Self::default();
        lock(&locator.vms).insert(LeaseId::new(lease), vm_name.to_string());
        Arc::new(locator)
    }

    pub fn set_ip(&self, vm_name: &str, ip: &str) {
        lock(&self.ips).insert(vm_name.to_string(), ip.to_string());
    }

    pub fn forget_after(&self, lookups: usize) {
        *lock(&self.lookups_left) = Some(lookups);
    }
}

#[async_trait]
impl GuestLocator for FakeLocator {
    async fn vm_name(&self, lease_id: &LeaseId) -> Option<String> {
        let mut left = lock(&self.lookups_left);
        if let Some(remaining) = left.as_mut() {
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
        }
        lock(&self.vms).get(lease_id).cloned()
    }

    async fn vmi_ip(&self, vm_name: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let ip = lock(&self.ips).get(vm_name).cloned();
        ip.ok_or_else(|| std::io::Error::other("the VMI reports no address").into())
    }
}

/// What the fake guest saw.
#[derive(Default)]
pub struct Seen {
    pub authorized: Vec<String>,
    pub ptys: Vec<(String, u32, u32)>,
    pub execs: Vec<String>,
    pub sizes: Vec<(u32, u32)>,
    pub closed_channels: usize,
    pub logins: usize,
}

/// How the fake guest misbehaves.
#[derive(Default)]
pub struct Faults {
    pub refuse_channels: AtomicBool,
}

/// The first two fields of an `authorized_keys` line, without the comment.
fn key_body(line: &str) -> String {
    line.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// An in-process SSH server standing in for a guest's `sshd`. A shell greets with `ready`, `echo X` answers `X`,
/// `exit` exits 3, and a resize answers with the new size.
pub struct GuestServer {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Seen>>,
    pub faults: Arc<Faults>,
    sessions: Arc<Mutex<Vec<russh::server::Handle>>>,
    accept: JoinHandle<()>,
}

impl GuestServer {
    pub async fn start() -> TestResult<Self> {
        Self::start_seeing(Arc::default()).await
    }

    /// Starts a server that shares `seen` (and so its authorized keys) with another.
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

    /// Disconnects every client, as a guest reboot would.
    pub async fn drop_sessions(&self) {
        let sessions: Vec<_> = lock(&self.sessions).drain(..).collect();
        for session in sessions {
            let _ = session
                .disconnect(Disconnect::ByApplication, "rebooting".to_string(), "en".to_string())
                .await;
        }
    }
}

impl Drop for GuestServer {
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
        let handler = GuestHandler {
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

struct GuestHandler {
    seen: Arc<Mutex<Seen>>,
    faults: Arc<Faults>,
    line: Vec<u8>,
}

impl Handler for GuestHandler {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let presented = format!("{} {}", key.algorithm().as_str(), key.public_key_base64());
        let mut seen = lock(&self.seen);
        let known = seen.authorized.iter().any(|line| key_body(line) == presented);
        if user == GUEST_USER && known {
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
        lock(&self.seen).ptys.push((term.to_string(), col_width, row_height));
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
            let line = String::from_utf8_lossy(&line).trim().to_string();
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

/// Points the SSH client at a [`GuestServer`] and records key installs. It holds `lease-1`.
pub struct FakeAccess {
    addr: Mutex<SocketAddr>,
    seen: Arc<Mutex<Seen>>,
    pub installs: AtomicUsize,
    /// Installs fail.
    pub fail_install: AtomicBool,
    /// Address lookups fail.
    pub fail_address: AtomicBool,
    /// Installs succeed without authorizing the key on the server.
    pub skip_authorize: AtomicBool,
}

impl FakeAccess {
    pub fn new(server: &GuestServer) -> Arc<Self> {
        Self::at(server.addr, Arc::clone(&server.seen))
    }

    pub fn at(addr: SocketAddr, seen: Arc<Mutex<Seen>>) -> Arc<Self> {
        Arc::new(Self {
            addr: Mutex::new(addr),
            seen,
            installs: AtomicUsize::new(0),
            fail_install: AtomicBool::new(false),
            fail_address: AtomicBool::new(false),
            skip_authorize: AtomicBool::new(false),
        })
    }

    pub fn point_at(&self, addr: SocketAddr) {
        *lock(&self.addr) = addr;
    }

    pub fn install_count(&self) -> usize {
        self.installs.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl GuestAccess for FakeAccess {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        lease_id.as_str() == "lease-1"
    }

    async fn address(&self, lease_id: &LeaseId) -> Result<SocketAddr, GuestAccessError> {
        if self.fail_address.load(Ordering::SeqCst) {
            return Err(GuestAccessError::Address {
                vm_name: lease_id.as_str().to_string(),
                source: std::io::Error::other("no address").into(),
            });
        }
        Ok(*lock(&self.addr))
    }

    async fn install_key(&self, lease_id: &LeaseId, public_key: &str) -> Result<(), GuestAccessError> {
        if self.fail_install.load(Ordering::SeqCst) {
            return Err(GuestAccessError::UnknownLease {
                lease_id: lease_id.clone(),
            });
        }
        self.installs.fetch_add(1, Ordering::SeqCst);
        if !self.skip_authorize.load(Ordering::SeqCst) {
            lock(&self.seen).authorized.push(public_key.to_string());
        }
        Ok(())
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
        Ok::<_, Box<dyn std::error::Error>>(seen)
    };
    tokio::time::timeout(Duration::from_secs(10), read).await?
}

/// Polls `check` until it holds, failing after ten seconds.
pub async fn eventually(mut check: impl FnMut() -> bool) -> TestResult {
    let wait = async {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        while !check() {
            tick.tick().await;
        }
    };
    Ok(tokio::time::timeout(ANSWER_TIMEOUT, wait).await?)
}

/// A NATS server running in a container, stopped when dropped.
pub struct Nats {
    inner: trogon_nats::test_support::CoreTestServer,
}

impl Nats {
    pub async fn start() -> Self {
        Self {
            inner: trogon_nats::test_support::CoreTestServer::start().await,
        }
    }

    /// Connects to the server, retrying for a while: the container's published port can refuse connections for a
    /// moment after the server reports it is listening.
    pub async fn client(&self) -> Result<async_nats::Client, async_nats::ConnectError> {
        let address = format!("nats://{}", self.inner.address());
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut attempts = 1;
        loop {
            tick.tick().await;
            match async_nats::connect(&address).await {
                Ok(client) => return Ok(client),
                Err(error) if attempts >= CONNECT_ATTEMPTS => return Err(error),
                Err(_) => attempts += 1,
            }
        }
    }
}

/// Sends `body` to `subject` until something answers, for handlers that subscribe in a spawned task.
///
/// A request that finds no subscriber yet is sent again. One that was delivered is waited for, never re-sent: a
/// second copy of a request a slow handler already served would be answered differently.
pub async fn request_until_answered(
    client: &async_nats::Client,
    subject: &str,
    body: Vec<u8>,
) -> TestResult<async_nats::Message> {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    for _ in 0..300 {
        tick.tick().await;
        let reply =
            tokio::time::timeout(ANSWER_TIMEOUT, client.request(subject.to_string(), body.clone().into())).await?;
        if let Ok(message) = reply {
            return Ok(message);
        }
    }
    Err(format!("no subscriber on {subject}").into())
}

/// Whether a request to `subject` goes unanswered for a short while.
pub async fn unanswered(client: &async_nats::Client, subject: &str, body: Vec<u8>) -> bool {
    let reply = tokio::time::timeout(
        Duration::from_millis(400),
        client.request(subject.to_string(), body.into()),
    )
    .await;
    !matches!(reply, Ok(Ok(_)))
}
