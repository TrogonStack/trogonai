//! Upstream dialing. When the transparent redirect is in play, every upstream
//! socket the proxy opens carries the egress netfilter mark, so the redirect
//! that captures the sandbox's traffic lets the proxy's own connections
//! through instead of looping them back into the proxy. Without a transparent
//! listener there is no redirect and no reason to need the privilege that
//! marking requires.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::time::timeout;
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::ProxyUid;
use trogon_sandbox_contract::{NetworkOptions, ParentProxyUrl, ProxyScheme};
use trogon_sandbox_protocol::{mitm_routes, parent_proxy_route_for};

use crate::parent_pool::{ParentPool, PoolKey};
use crate::resolve::VettedEndpoint;
pub use crate::upstream_stream::UpstreamStream;

mod constants;
pub use constants::UPSTREAM_MARK;
use constants::{CONNECT_TIMEOUT, MAX_CONNECT_RESPONSE_HEAD};

/// Whether upstream sockets must carry [`UPSTREAM_MARK`], decided once at
/// startup from whether a transparent listener exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamMarking {
    /// A transparent redirect is installed; an unmarked dial would loop.
    Required,
    /// No transparent redirect; dials are left unmarked.
    NotNeeded,
}

impl UpstreamMarking {
    pub fn for_transparent_listener(enabled: bool) -> Self {
        if enabled { Self::Required } else { Self::NotNeeded }
    }

    /// Fails when marking is required and this process cannot set the mark
    /// (it needs `CAP_NET_ADMIN`), which would otherwise surface only as
    /// every upstream dial failing.
    pub fn verify(self) -> io::Result<()> {
        match self {
            Self::Required => marked_socket(SocketAddr::from(([127, 0, 0, 1], 0)), self).map(drop),
            Self::NotNeeded => Ok(()),
        }
    }
}

/// How the transparent redirect recognises the proxy's own upstream traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamExemption {
    /// Upstream sockets carry [`UPSTREAM_MARK`]; needs `CAP_NET_ADMIN`.
    Mark,
    /// The redirect exempts this process by `--uid-owner`; needs no capability.
    Uid(ProxyUid),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpstreamExemptionError {
    #[error("{value:?} is not a valid uid")]
    InvalidUid { value: String },
    #[error("the redirect exempts uid {exempt} but this process runs as uid {actual}")]
    UidMismatch { exempt: ProxyUid, actual: ProxyUid },
}

impl UpstreamExemption {
    /// `None` selects [`Self::Mark`]; a value selects [`Self::Uid`].
    pub fn from_exempt_uid(value: Option<&str>) -> Result<Self, UpstreamExemptionError> {
        match value {
            None => Ok(Self::Mark),
            Some(value) => value
                .trim()
                .parse()
                .map(Self::Uid)
                .map_err(|_| UpstreamExemptionError::InvalidUid {
                    value: value.to_string(),
                }),
        }
    }

    pub fn marking_for_transparent_listener(self, enabled: bool) -> UpstreamMarking {
        match self {
            Self::Mark => UpstreamMarking::for_transparent_listener(enabled),
            Self::Uid(_) => UpstreamMarking::NotNeeded,
        }
    }

    /// Fails when the exemption is by uid and `actual` is not that uid, which
    /// would otherwise surface only as every upstream dial looping back into
    /// the proxy.
    pub fn verify_uid(self, actual: ProxyUid) -> Result<(), UpstreamExemptionError> {
        match self {
            Self::Uid(exempt) if exempt != actual => Err(UpstreamExemptionError::UidMismatch { exempt, actual }),
            Self::Uid(_) | Self::Mark => Ok(()),
        }
    }
}

#[cfg(unix)]
pub fn effective_uid() -> io::Result<ProxyUid> {
    Ok(ProxyUid::new(unsafe { libc::geteuid() }))
}

#[cfg(not(unix))]
pub fn effective_uid() -> io::Result<ProxyUid> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "uid-based upstream exemption needs a unix platform",
    ))
}

/// Connects to the vetted endpoint, the only address a caller may dial.
pub async fn connect(marking: UpstreamMarking, endpoint: VettedEndpoint) -> io::Result<TcpStream> {
    if !endpoint.addr().is_vetted() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the destination was not resolved",
        ));
    }
    dial(marking, endpoint.socket_addr()).await
}

/// Connects to a destination the policy author declared (a `Forward` rule's
/// target), which is not sandbox-controlled and so not subject to the floor.
pub async fn connect_declared(marking: UpstreamMarking, host: &str, port: u16) -> io::Result<TcpStream> {
    let addr = tokio::net::lookup_host((host, port))
        .await?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no addresses"))?;
    dial(marking, addr).await
}

async fn dial(marking: UpstreamMarking, addr: SocketAddr) -> io::Result<TcpStream> {
    marked_socket(addr, marking)?.connect(addr).await
}

fn marked_socket(addr: SocketAddr, marking: UpstreamMarking) -> io::Result<TcpSocket> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    if marking == UpstreamMarking::Required {
        apply_mark(&socket)?;
    }
    Ok(socket)
}

#[cfg(target_os = "linux")]
fn apply_mark(socket: &TcpSocket) -> io::Result<()> {
    socket2::SockRef::from(socket).set_mark(UPSTREAM_MARK.get())
}

#[cfg(not(target_os = "linux"))]
fn apply_mark(_socket: &TcpSocket) -> io::Result<()> {
    Ok(())
}

fn refused(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, message.into())
}

/// Asks the proxy on `stream` for a tunnel to `host:port` and leaves the
/// stream positioned at the first tunnelled byte. The host is validated
/// before anything is written, so a name carrying line breaks never reaches
/// the proxy.
pub async fn open_connect_tunnel<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    authorization: Option<&str>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let host = Host::new(host).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid destination host for CONNECT: {host:?}"),
        )
    })?;
    if port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid destination port: 0",
        ));
    }
    let authority = format!("{host}:{port}");
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(authorization) = authorization {
        request.push_str(&format!("Proxy-Authorization: {authorization}\r\n"));
    }
    request.push_str("\r\n");

    timeout(CONNECT_TIMEOUT, async {
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() >= MAX_CONNECT_RESPONSE_HEAD {
                return Err(refused("CONNECT response header too large"));
            }
            if stream.read(&mut byte).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "proxy closed during CONNECT handshake",
                ));
            }
            head.push(byte[0]);
        }
        let status_line = head.split(|byte| *byte == b'\r').next().unwrap_or_default();
        let status_line = String::from_utf8_lossy(status_line);
        let mut parts = status_line.split_whitespace();
        let version_ok = matches!(parts.next(), Some("HTTP/1.0" | "HTTP/1.1"));
        let success = parts
            .next()
            .is_some_and(|code| code.len() == 3 && code.starts_with('2'));
        if version_ok && success {
            Ok(())
        } else {
            Err(refused(format!("proxy refused CONNECT: {}", status_line.trim())))
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "CONNECT handshake timed out"))?
}

#[cfg(unix)]
async fn connect_unix(path: &Path) -> io::Result<UpstreamStream> {
    Ok(UpstreamStream::Unix(tokio::net::UnixStream::connect(path).await?))
}

#[cfg(not(unix))]
async fn connect_unix(_path: &Path) -> io::Result<UpstreamStream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "a MITM unix socket needs a unix platform",
    ))
}

/// Where a destination is reached from: the options say whether a MITM unix
/// socket or a parent proxy sits in between, and `tls_config` secures the hop
/// to an https parent proxy.
pub struct UpstreamRoutes<'a> {
    pub marking: UpstreamMarking,
    pub options: &'a NetworkOptions,
    pub tls_config: &'a Arc<rustls::ClientConfig>,
    pub parent_pool: &'a ParentPool,
}

enum Hop<'a> {
    Mitm(&'a Path),
    Parent(&'a ParentProxyUrl),
    Direct,
}

/// An upstream opened for one plain-HTTP request: through a MITM socket or a
/// parent proxy the request is sent in absolute form, with the parent's
/// credentials, instead of inside a tunnel. A parent's connection goes back
/// to the pool it came from once a response leaves it reusable.
pub struct PlainUpstream {
    pub stream: UpstreamStream,
    pub absolute_form: bool,
    pub proxy_authorization: Option<String>,
    pub returns_to: Option<PoolKey>,
}

impl PlainUpstream {
    pub fn keeps_alive(&self) -> bool {
        self.returns_to.is_some()
    }
}

fn route<'a>(options: &'a NetworkOptions, host: &Host, tls: bool, port: u16) -> Hop<'a> {
    if let Some(mitm) = &options.mitm
        && mitm_routes(mitm, host, port)
    {
        return Hop::Mitm(&mitm.socket_path);
    }
    if let Some(parent) = &options.parent_proxy
        && let Some(proxy) =
            parent_proxy_route_for(parent, host, if tls { ProxyScheme::Https } else { ProxyScheme::Http })
    {
        return Hop::Parent(proxy);
    }
    Hop::Direct
}

/// Whether the connection to `host` goes through the MITM socket or a parent
/// proxy, which are handed the name and resolve it themselves.
pub(crate) fn carries_name(options: &NetworkOptions, host: &Host, tls: bool, port: u16) -> bool {
    !matches!(route(options, host, tls, port), Hop::Direct)
}

impl UpstreamRoutes<'_> {
    fn hop(&self, name: &str, tls: bool, port: u16) -> Hop<'_> {
        match Host::new(name) {
            Ok(host) => route(self.options, &host, tls, port),
            Err(_) => Hop::Direct,
        }
    }

    async fn reach_parent(&self, proxy: &ParentProxyUrl) -> io::Result<UpstreamStream> {
        let proxy_host = proxy.host().to_string();
        let dial_host = proxy_host.trim_matches(['[', ']']);
        let stream = connect_declared(self.marking, dial_host, proxy.port().get()).await?;
        match proxy.scheme() {
            ProxyScheme::Http => Ok(UpstreamStream::Tcp(stream)),
            ProxyScheme::Https => {
                let server_name = rustls::pki_types::ServerName::try_from(dial_host.to_string())
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
                let secured = tokio_rustls::TlsConnector::from(Arc::clone(self.tls_config))
                    .connect(server_name, stream)
                    .await?;
                Ok(UpstreamStream::Tls(Box::new(secured)))
            }
        }
    }

    /// Opens the upstream for `target` (the name the client asked for, or the
    /// vetted address when it asked for none). The route is the MITM unix
    /// socket when it claims the destination, else the parent proxy unless
    /// bypassed, else a direct dial of the vetted endpoint.
    pub async fn connect(
        &self,
        target: Option<&str>,
        tls: bool,
        endpoint: VettedEndpoint,
    ) -> io::Result<UpstreamStream> {
        let ip = endpoint.addr().ip().to_string();
        let name = target.unwrap_or(&ip);
        let port = endpoint.port();
        match self.hop(name, tls, port) {
            Hop::Mitm(socket) => {
                let mut stream = connect_unix(socket).await?;
                open_connect_tunnel(&mut stream, name, port, None).await?;
                Ok(stream)
            }
            Hop::Parent(proxy) => {
                let mut stream = self.reach_parent(proxy).await?;
                open_connect_tunnel(&mut stream, name, port, proxy.authorization()).await?;
                Ok(stream)
            }
            Hop::Direct => connect(self.marking, endpoint).await.map(UpstreamStream::Tcp),
        }
    }

    /// [`Self::connect`] over the vetted endpoints: a direct dial takes them
    /// in address-family order until one connects, a MITM socket or parent proxy,
    /// which does not depend on the endpoint, is reached once.
    pub async fn connect_any(
        &self,
        target: Option<&str>,
        tls: bool,
        endpoints: &[VettedEndpoint],
    ) -> io::Result<UpstreamStream> {
        let first = self.first_endpoint(endpoints)?;
        let ip = first.addr().ip().to_string();
        match self.hop(target.unwrap_or(&ip), tls, first.port()) {
            Hop::Direct => {
                crate::address_turns::first_to_connect(endpoints, |endpoint| connect(self.marking, endpoint))
                    .await
                    .map(UpstreamStream::Tcp)
            }
            _ => self.connect(target, tls, first).await,
        }
    }

    /// [`Self::connect_any`] for a plain-HTTP request.
    pub async fn connect_plain_any(
        &self,
        target: Option<&str>,
        endpoints: &[VettedEndpoint],
    ) -> io::Result<PlainUpstream> {
        let first = self.first_endpoint(endpoints)?;
        let ip = first.addr().ip().to_string();
        match self.hop(target.unwrap_or(&ip), false, first.port()) {
            Hop::Direct => {
                let stream =
                    crate::address_turns::first_to_connect(endpoints, |endpoint| connect(self.marking, endpoint))
                        .await?;
                let stream = match target {
                    Some(name) if !is_ip_literal(name) => self.reconnect(endpoints, stream).await?,
                    _ => stream,
                };
                Ok(PlainUpstream {
                    stream: UpstreamStream::Tcp(stream),
                    absolute_form: false,
                    proxy_authorization: None,
                    returns_to: None,
                })
            }
            _ => self.connect_plain(target, first).await,
        }
    }

    fn first_endpoint(&self, endpoints: &[VettedEndpoint]) -> io::Result<VettedEndpoint> {
        endpoints
            .first()
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no addresses"))
    }

    /// [`Self::connect`] for a plain-HTTP request: a MITM socket or parent
    /// proxy is asked for the request itself, not for a tunnel to port 80.
    pub async fn connect_plain(&self, target: Option<&str>, endpoint: VettedEndpoint) -> io::Result<PlainUpstream> {
        let ip = endpoint.addr().ip().to_string();
        let name = target.unwrap_or(&ip);
        match self.hop(name, false, endpoint.port()) {
            Hop::Mitm(socket) => Ok(PlainUpstream {
                stream: connect_unix(socket).await?,
                absolute_form: true,
                proxy_authorization: None,
                returns_to: None,
            }),
            Hop::Parent(proxy) => {
                let key = PoolKey::from(proxy);
                let stream = match self.parent_pool.take(&key) {
                    Some(stream) => stream,
                    None => self.reach_parent(proxy).await?,
                };
                Ok(PlainUpstream {
                    stream,
                    absolute_form: true,
                    proxy_authorization: proxy.authorization().map(str::to_string),
                    returns_to: Some(key),
                })
            }
            Hop::Direct => Ok(PlainUpstream {
                stream: connect(self.marking, endpoint).await.map(UpstreamStream::Tcp)?,
                absolute_form: false,
                proxy_authorization: None,
                returns_to: None,
            }),
        }
    }

    /// The connection a name was vetted with is only a probe: it is closed
    /// and the literal address that answered is requested over a new one.
    async fn reconnect(&self, endpoints: &[VettedEndpoint], probe: TcpStream) -> io::Result<TcpStream> {
        let endpoint = answered(endpoints, probe)?;
        connect(self.marking, endpoint).await
    }
}

/// The endpoint a probe connection reached, which is requested by its literal
/// once the probe is closed.
fn answered(endpoints: &[VettedEndpoint], probe: TcpStream) -> io::Result<VettedEndpoint> {
    let answered = probe.peer_addr()?.ip();
    drop(probe);
    endpoints
        .iter()
        .find(|endpoint| endpoint.addr().ip() == answered)
        .or(endpoints.first())
        .copied()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no addresses"))
}

/// Whether a terminated tunnel's upstream leg vets its target with a probe
/// connection before dialing it: done for a name, never for an IP
/// literal.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LegVetting {
    Probe,
    AsGiven,
}

/// The vetted endpoints a terminated tunnel's upstream leg is dialed on.
#[derive(Debug, Clone)]
pub struct LegAddresses {
    first: VettedEndpoint,
    rest: Vec<VettedEndpoint>,
    vetting: LegVetting,
}

impl LegAddresses {
    pub fn new(first: VettedEndpoint, rest: Vec<VettedEndpoint>, vetting: LegVetting) -> Self {
        Self { first, rest, vetting }
    }

    pub fn first(&self) -> VettedEndpoint {
        self.first
    }

    pub fn port(&self) -> u16 {
        self.first.port()
    }

    pub(crate) fn endpoints(&self) -> Vec<VettedEndpoint> {
        std::iter::once(self.first).chain(self.rest.iter().copied()).collect()
    }
}

impl From<VettedEndpoint> for LegAddresses {
    fn from(endpoint: VettedEndpoint) -> Self {
        Self::new(endpoint, Vec::new(), LegVetting::AsGiven)
    }
}

/// Dials a terminated tunnel's upstream leg a name is
/// vetted by a probe connection once per leg, every dial after that, a
/// redial after the upstream closed an idle connection included, goes to the
/// endpoint that answered, and a failed dial forgets it so the next one
/// vets again.
pub(crate) struct LegDialer {
    addresses: LegAddresses,
    marking: UpstreamMarking,
    vetted: std::sync::Mutex<Option<VettedEndpoint>>,
}

impl LegDialer {
    pub(crate) fn new(addresses: LegAddresses, marking: UpstreamMarking) -> Self {
        Self {
            addresses,
            marking,
            vetted: std::sync::Mutex::new(None),
        }
    }

    pub(crate) async fn connect(&self) -> io::Result<TcpStream> {
        let endpoint = match self.addresses.vetting {
            LegVetting::AsGiven => self.addresses.first(),
            LegVetting::Probe => match self.remembered() {
                Some(endpoint) => endpoint,
                None => self.vet().await?,
            },
        };
        connect(self.marking, endpoint).await
    }

    pub(crate) fn forget(&self) {
        *self.vetted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn remembered(&self) -> Option<VettedEndpoint> {
        *self.vetted.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn vet(&self) -> io::Result<VettedEndpoint> {
        let endpoints = self.addresses.endpoints();
        let probe =
            crate::address_turns::first_to_connect(&endpoints, |endpoint| connect(self.marking, endpoint)).await?;
        let endpoint = answered(&endpoints, probe)?;
        *self.vetted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(endpoint);
        Ok(endpoint)
    }
}

fn is_ip_literal(name: &str) -> bool {
    name.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests;
