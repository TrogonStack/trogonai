//! Reaching a TCP port a lease's process listens on.
//!
//! Under bubblewrap a lease with a private network (`--unshare-net`, which is
//! every lease behind the egress relay or with egress denied) has its own
//! loopback, so the worker host cannot dial a server the lease starts. The
//! worker therefore opens a loopback listener on the host per requested port
//! and bridges each accepted connection through a short-lived helper process
//! that enters the lease's user and network namespaces and dials
//! `127.0.0.1:<port>` there, the way the docker worker bridges through
//! `docker exec`. The worker binary doubles as the helper (`--port-bridge`).
//!
//! A lease that shares the host network (egress allowed in full), and every
//! lease on the macOS and unconfined backends, has no namespace to enter: a
//! port it opens is a port on the host, which cannot be told apart from the
//! host's own services, so those leases answer `Unsupported`.
//!
//! Terminals have separate owned namespaces. Connections are probed without
//! client bytes and forwarded only when exactly one live namespace accepts.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::sync::Weak;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseError, LeaseId};

pub use crate::bridge_io::{BridgeIo, Connector};
pub use crate::constants::{MAX_BRIDGES_PER_FORWARD, MAX_FORWARDS_PER_LEASE};

/// Ports of the egress relay inside the sandbox. They reach the lease's proxy,
/// not a server the lease runs, so they are never exposed.
#[cfg(target_os = "linux")]
fn reserved_ports() -> [u16; 2] {
    [
        crate::isolation::relay::RELAY_PORT,
        crate::isolation::relay::RELAY_SOCKS_PORT,
    ]
}

#[cfg(not(target_os = "linux"))]
fn reserved_ports() -> [u16; 0] {
    []
}

/// A loopback listener forwarding to one port of a lease. Dropping it stops
/// accepting and ends the bridges already open.
pub struct PortForward {
    pub endpoint: String,
    stop: CancellationToken,
    accept_loop: JoinHandle<()>,
}

impl Drop for PortForward {
    fn drop(&mut self) {
        self.stop.cancel();
        self.accept_loop.abort();
    }
}

impl PortForward {
    pub async fn start(connector: Arc<dyn Connector>, port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?.to_string();
        let stop = CancellationToken::new();
        let permits = Arc::new(Semaphore::new(MAX_BRIDGES_PER_FORWARD));
        let accept_loop = tokio::spawn({
            let stop = stop.clone();
            async move {
                loop {
                    let stream = match listener.accept().await {
                        Ok((stream, _)) => stream,
                        Err(e) => {
                            warn!(error = %e, port, "port forward accept failed");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    };
                    // Over the cap the connection is dropped, which the
                    // client sees as a reset.
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        debug!(port, "port forward at its connection cap");
                        continue;
                    };
                    let connector = Arc::clone(&connector);
                    let stop = stop.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        tokio::select! {
                            _ = stop.cancelled() => {}
                            result = bridge(connector.as_ref(), port, stream) => {
                                if let Err(e) = result {
                                    debug!(error = %e, port, "port forward bridge ended");
                                }
                            }
                        }
                    });
                }
            }
        });
        Ok(Self {
            endpoint,
            stop,
            accept_loop,
        })
    }
}

/// Carries `client` to the lease's port. The lease side closing ends the
/// connection; the client closing only half-closes, so the reply can drain.
async fn bridge(connector: &dyn Connector, port: u16, client: TcpStream) -> io::Result<()> {
    let upstream = connector.connect(port).await?;
    let (mut lease_read, mut lease_write) = tokio::io::split(upstream);
    let (mut client_read, mut client_write) = client.into_split();
    let to_lease = async {
        tokio::io::copy(&mut client_read, &mut lease_write).await?;
        lease_write.shutdown().await
    };
    let to_client = async {
        tokio::io::copy(&mut lease_read, &mut client_write).await?;
        client_write.shutdown().await
    };
    tokio::pin!(to_lease);
    tokio::pin!(to_client);
    let mut to_lease_done = false;
    loop {
        tokio::select! {
            up = &mut to_lease, if !to_lease_done => {
                to_lease_done = true;
                up?;
            }
            down = &mut to_client => return down,
        }
    }
}

/// How a lease's ports can be reached.
pub enum Reach {
    /// The lease runs under bubblewrap in a private network namespace; its
    /// namespaces are found from `pid`, the bubblewrap process, on first use.
    /// `helper` is the binary that handles `--port-bridge`; the running
    /// executable when absent.
    #[cfg(target_os = "linux")]
    Process {
        pid: u32,
        helper: Option<std::path::PathBuf>,
    },
    /// A connector supplied directly.
    Connector(Arc<dyn Connector>),
    /// The lease has no network namespace of its own to enter.
    Unsupported,
}

#[derive(Clone)]
struct Candidate {
    connector: Arc<dyn Connector>,
    stop: CancellationToken,
}

struct PreviewCandidates {
    primary: Candidate,
    terminals: HashMap<u64, Candidate>,
    generation: u64,
    #[cfg(any(target_os = "linux", test))]
    next: u64,
}

/// Authority retained by one registration, independent of reuse of its lease ID.
#[derive(Clone)]
pub(crate) struct LeasePreview {
    candidates: Arc<StdMutex<PreviewCandidates>>,
    stop: CancellationToken,
    #[cfg(target_os = "linux")]
    helper: Option<std::path::PathBuf>,
}

impl LeasePreview {
    fn new(connector: Arc<dyn Connector>) -> Self {
        let stop = CancellationToken::new();
        Self {
            candidates: Arc::new(StdMutex::new(PreviewCandidates {
                primary: Candidate {
                    connector,
                    stop: stop.clone(),
                },
                terminals: HashMap::new(),
                generation: 0,
                #[cfg(any(target_os = "linux", test))]
                next: 0,
            })),
            stop,
            #[cfg(target_os = "linux")]
            helper: None,
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) async fn register_process(
        &self,
        pid: u32,
        child: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    ) -> io::Result<Option<PreviewMembership>> {
        let helper = self
            .helper
            .clone()
            .ok_or_else(|| io::Error::other("preview has no owned namespace helper"))?;
        let mut child = child.lock().await;
        if child.as_ref().and_then(tokio::process::Child::id) != Some(pid) {
            return Err(io::Error::other("terminal process ownership is unavailable"));
        }
        let connector = match crate::namespace_bridge::NamespaceTarget::open(pid, helper) {
            Ok(connector) => connector,
            Err(error) => {
                if child
                    .as_mut()
                    .ok_or_else(|| io::Error::other("terminal child is unavailable"))?
                    .try_wait()?
                    .is_some()
                {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        self.register_candidate(Arc::new(connector)).map(Some)
    }

    #[cfg(any(target_os = "linux", test))]
    fn register_candidate(&self, connector: Arc<dyn Connector>) -> io::Result<PreviewMembership> {
        let mut candidates = self
            .candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.stop.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "lease preview is revoked"));
        }
        if candidates.terminals.len() >= trogon_sandbox_protocol::MAX_TERMINALS_PER_LEASE {
            return Err(io::Error::other("lease preview namespace limit reached"));
        }
        let id = candidates.next;
        candidates.next = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("preview identity exhausted"))?;
        candidates.generation = candidates
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("preview generation exhausted"))?;
        let stop = self.stop.child_token();
        candidates.terminals.insert(
            id,
            Candidate {
                connector,
                stop: stop.clone(),
            },
        );
        Ok(PreviewMembership {
            candidates: Arc::downgrade(&self.candidates),
            id,
            stop,
        })
    }

    pub(crate) fn revoke(&self) {
        self.stop.cancel();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", test))]
    pub(crate) fn is_revoked(&self) -> bool {
        self.stop.is_cancelled()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub(crate) struct PreviewMembership {
    candidates: Weak<StdMutex<PreviewCandidates>>,
    id: u64,
    stop: CancellationToken,
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
impl Drop for PreviewMembership {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(candidates) = self.candidates.upgrade() {
            let mut candidates = candidates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if candidates.terminals.remove(&self.id).is_some() {
                match candidates.generation.checked_add(1) {
                    Some(next) => candidates.generation = next,
                    None => candidates.primary.stop.cancel(),
                }
            }
        }
    }
}

#[async_trait]
impl Connector for LeasePreview {
    async fn connect(&self, port: u16) -> io::Result<Box<dyn BridgeIo>> {
        let (generation, candidates) = {
            let candidates = self
                .candidates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                candidates.generation,
                std::iter::once(candidates.primary.clone())
                    .chain(candidates.terminals.values().cloned())
                    .collect::<Vec<_>>(),
            )
        };
        let mut selected = None;
        for candidate in candidates {
            let result = tokio::select! {
                _ = candidate.stop.cancelled() => return Err(io::ErrorKind::ConnectionAborted.into()),
                result = tokio::time::timeout(Duration::from_secs(3), candidate.connector.connect(port)) => {
                    result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "owned namespace probe timed out"))?
                }
            };
            match result {
                Ok(stream) => {
                    if selected.is_some() {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "port is ambiguous across owned namespaces",
                        ));
                    }
                    selected = Some((stream, candidate.stop));
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
                Err(error) => return Err(error),
            }
        }
        let candidates = self
            .candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.stop.is_cancelled() || candidates.generation != generation {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "owned preview namespace membership changed",
            ));
        }
        let (stream, stop) = selected.ok_or(io::ErrorKind::ConnectionRefused)?;
        Ok(Box::new(PreviewConnection {
            stream,
            revoked: Box::pin(stop.cancelled_owned()),
        }))
    }
}

struct PreviewConnection {
    stream: Box<dyn BridgeIo>,
    revoked: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl AsyncRead for PreviewConnection {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.revoked.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for PreviewConnection {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if self.revoked.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.revoked.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

enum RegisteredReach {
    Ready(LeasePreview),
    #[cfg(target_os = "linux")]
    Failed(String),
    Unsupported,
}

struct LeaseForwards {
    reach: RegisteredReach,
    forwards: HashMap<u16, PortForward>,
}

impl Drop for LeaseForwards {
    fn drop(&mut self) {
        if let RegisteredReach::Ready(preview) = &self.reach {
            preview.revoke();
        }
    }
}

/// The leases this worker runs and the forwards opened into them. Shared
/// between the launcher, which registers and removes leases, and the
/// `get_domain` handler.
#[derive(Clone, Default)]
pub struct PortForwardRegistry {
    leases: Arc<Mutex<HashMap<String, LeaseForwards>>>,
}

impl PortForwardRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, lease_id: &LeaseId, reach: Reach) {
        self.register_with_preview(lease_id, reach).await;
    }

    pub(crate) async fn register_with_preview(&self, lease_id: &LeaseId, reach: Reach) -> Option<LeasePreview> {
        let reach = match reach {
            Reach::Connector(connector) => RegisteredReach::Ready(LeasePreview::new(connector)),
            #[cfg(target_os = "linux")]
            Reach::Process { pid, helper } => {
                let helper = helper.map_or_else(std::env::current_exe, Ok);
                match helper.and_then(|helper| {
                    let connector = crate::namespace_bridge::NamespaceTarget::open(pid, helper.clone())?;
                    let mut preview = LeasePreview::new(Arc::new(connector));
                    preview.helper = Some(helper);
                    Ok(preview)
                }) {
                    Ok(preview) => RegisteredReach::Ready(preview),
                    Err(error) => RegisteredReach::Failed(error.to_string()),
                }
            }
            Reach::Unsupported => RegisteredReach::Unsupported,
        };
        let preview = match &reach {
            RegisteredReach::Ready(preview) => Some(preview.clone()),
            _ => None,
        };
        self.leases.lock().await.insert(
            lease_id.to_string(),
            LeaseForwards {
                reach,
                forwards: HashMap::new(),
            },
        );
        preview
    }

    /// Forgets the lease and closes its forwards.
    pub async fn remove(&self, lease_id: &LeaseId) {
        self.leases.lock().await.remove(lease_id.as_str());
    }

    /// Whether this worker runs `lease_id`; only that worker answers.
    pub async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.leases.lock().await.contains_key(lease_id.as_str())
    }

    pub async fn resolve(&self, lease_id: &LeaseId, port: u16) -> GetSandboxDomainResponse {
        let available = |endpoint: String| GetSandboxDomainResponse::Available {
            endpoint,
            url: None,
            expires_at: None,
            access: None,
        };
        // The lock is held across the bind, so a release cannot slip between
        // the check that the lease exists and the insert of its forward: it
        // either runs first and the lease is gone, or after and drops the
        // forward with the rest.
        let mut leases = self.leases.lock().await;
        let Some(lease) = leases.get_mut(lease_id.as_str()) else {
            return GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            });
        };
        if matches!(lease.reach, RegisteredReach::Unsupported) {
            return GetSandboxDomainResponse::Unsupported;
        }
        #[cfg(target_os = "linux")]
        if let RegisteredReach::Failed(message) = &lease.reach {
            return GetSandboxDomainResponse::Denied(LeaseError::Backend {
                message: message.clone(),
            });
        }
        if port == 0 || reserved_ports().contains(&port) {
            return GetSandboxDomainResponse::PortNotExposed;
        }
        if let Some(forward) = lease.forwards.get(&port) {
            return available(forward.endpoint.clone());
        }
        if lease.forwards.len() >= MAX_FORWARDS_PER_LEASE {
            return GetSandboxDomainResponse::PortNotExposed;
        }
        let connector: Arc<dyn Connector> = match &lease.reach {
            RegisteredReach::Ready(preview) => Arc::new(preview.clone()),
            _ => unreachable!("unsupported and failed preview reach were handled above"),
        };
        match PortForward::start(connector, port).await {
            Ok(forward) => {
                let endpoint = forward.endpoint.clone();
                lease.forwards.insert(port, forward);
                available(endpoint)
            }
            Err(e) => GetSandboxDomainResponse::Denied(LeaseError::Backend {
                message: format!("could not open a forward for port {port}: {e}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests;
