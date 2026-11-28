//! How long a terminated tunnel lives, through the whole proxy: the
//! handshake deadline bounds the handshake and nothing after it, and a tunnel
//! held in its handshake keeps its slot only until the deadline; a client
//! that resets or drops its tunnel, or sends it garbage, ends that tunnel
//! alone.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rustls::SupportedProtocolVersion;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tokio_rustls::TlsConnector;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{AllowRange, DomainRule, DomainRuleAction, LeaseId, NetworkPolicy};
use trogon_sandbox_contract::{AllowedTlsTermination, NetworkOptions, TunnelCap, UpstreamCaCertificates};
use trogon_sandbox_egress_proxy::TunnelGate;
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer};
use trogon_sandbox_egress_proxy::tls_ca::{
    PermittedDomains, SandboxCa, install_default_crypto_provider, server_config_for_leaf,
};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const TOKEN: &str = "lease-token-value";
const HOST: &str = "allowed.test";
/// The deadline of the cases that measure it. Long enough that a handshake
/// on a machine under heavy load still fits inside it.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// The deadline of the cases that do not measure it: it must never fire.
const UNTIMED: Duration = Duration::from_secs(120);
/// How long any one step of a case may take before the case fails loudly,
/// whatever the load.
const STEP: Duration = Duration::from_secs(60);
/// Timers do not fire early; the proxy arms its timer just before the
/// client sees the 200.
const EARLY: Duration = Duration::from_millis(100);

/// `step`, or an error naming `what` once [`STEP`] has passed, so that a case
/// never waits on something that will not happen.
async fn bounded<T>(what: &str, step: impl std::future::Future<Output = T>) -> TestResult<T> {
    match timeout(STEP, step).await {
        Ok(done) => Ok(done),
        Err(_) => Err(format!("{what} did not finish within {STEP:?}").into()),
    }
}

/// A copy of the request targets the upstream has seen.
fn targets(seen: &Mutex<Vec<String>>) -> TestResult<Vec<String>> {
    Ok(seen.lock().map_err(|_| "seen lock poisoned")?.clone())
}

struct LoopbackLookup;

#[async_trait]
impl NameLookup for LoopbackLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
    }
}

struct DroppedViolations;

#[async_trait]
impl ViolationSink for DroppedViolations {
    async fn publish(&self, _subject: String, _payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        Ok(())
    }
}

/// A proxy that terminates TLS on one tunnel at a time, with a short
/// handshake deadline, in front of an HTTPS upstream on loopback.
struct Serve {
    server: ProxyServer,
    proxy: SocketAddr,
    target: String,
    roots: rustls::RootCertStore,
    /// The request targets the upstream has seen, in order.
    seen: Arc<Mutex<Vec<String>>>,
}

type ClientTls = tokio_rustls::client::TlsStream<TcpStream>;

/// A tunnel the proxy answered 200, and when it did.
struct Tunnel {
    raw: TcpStream,
    opened_at: Instant,
}

impl Serve {
    async fn start(handshake: Duration) -> TestResult<Self> {
        install_default_crypto_provider();
        let rules = vec![DomainRule {
            domain: HOST.parse()?,
            action: DomainRuleAction::PassThrough,
        }];
        let upstream_ca = SandboxCa::generate(PermittedDomains::from_rules(&rules)?)?;
        let acceptor = tokio_rustls::TlsAcceptor::from(server_config_for_leaf(upstream_ca.mint_leaf_cert(HOST)?));
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let port = upstream.local_addr()?.port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((stream, _)) = upstream.accept().await {
                let acceptor = acceptor.clone();
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    while let Some(head) = read_head(&mut tls).await {
                        let target = head.split(' ').nth(1).map_or("", |part| part).to_owned();
                        if let Ok(mut targets) = recorded.lock() {
                            targets.push(target);
                        }
                        if tls
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        let policy = NetworkPolicy::UserDefined {
            rules: rules.clone(),
            allow_ranges: vec![AllowRange::new("127.0.0.0/8")?],
            deny_ranges: vec![],
            denied_domains: vec![],
            opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
            request_rules: vec![],
        };
        let options = NetworkOptions {
            terminate_allowed_tls: AllowedTlsTermination::Enabled,
            upstream_ca: UpstreamCaCertificates::new([upstream_ca.root_cert_pem()]),
            ..NetworkOptions::default()
        };
        let gate = Arc::new(TunnelGate::new(Some(TunnelCap::new(1)?), Some(handshake)));
        let ca = SandboxCa::generate(PermittedDomains::from_rules(&rules)?)?;
        let server = ProxyServer::builder(policy, ca)
            .explicit(
                Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?,
                ProxyToken::new(TOKEN)?,
            )
            .violations(Arc::new(ViolationReporter::new(
                LeaseId::new("L1"),
                Arc::new(DroppedViolations),
                ViolationRateLimit::default(),
            )))
            .resolver(Arc::new(VettingResolver::new(
                Arc::new(LoopbackLookup),
                SsrfFloor::system(),
            )))
            .network_options(options)
            .tunnel_gate(Arc::clone(&gate))
            .start()?;
        let proxy = match server.explicit_addr() {
            Some(BoundAddress::Tcp(addr)) => *addr,
            other => return Err(format!("{other:?}").into()),
        };
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut io::Cursor::new(server.ca_certificate_pem().as_bytes().to_vec())) {
            roots.add(cert?)?;
        }
        let serve = Self {
            server,
            proxy,
            target: format!("{HOST}:{port}"),
            roots,
            seen,
        };
        // Minting the first leaf certificate is slow on a busy machine, and
        // the handshake deadline is read at each CONNECT. One full exchange
        // under a patient deadline caches the leaf, so that the deadline a
        // case measures meets a handshake that takes no time worth counting.
        gate.set_handshake(Some(STEP));
        serve.send("/warm-up").await?;
        gate.set_handshake(Some(handshake));
        serve
            .seen
            .lock()
            .map_err(|_| "the recorded targets are poisoned")?
            .clear();
        Ok(serve)
    }

    /// The status line of the answer to a CONNECT, and the tunnel.
    async fn connect_to(&self) -> TestResult<(String, Tunnel)> {
        let mut raw = bounded("connecting to the proxy", TcpStream::connect(self.proxy)).await??;
        let authorization = STANDARD.encode(format!("sandbox:{TOKEN}"));
        let request = format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {authorization}\r\n\r\n",
            target = self.target,
        );
        bounded("sending the CONNECT", raw.write_all(request.as_bytes())).await??;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match bounded("reading the CONNECT answer", raw.read(&mut byte)).await? {
                Ok(1..) => head.push(byte[0]),
                _ => break,
            }
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let status = head.lines().next().map_or("", |line| line).to_owned();
        Ok((
            status,
            Tunnel {
                raw,
                opened_at: Instant::now(),
            },
        ))
    }

    /// A tunnel the proxy answers 200. The cap is one, and the slot of a
    /// tunnel that just ended is given back by the proxy's own task, so a 503
    /// is asked again until the slot is free.
    async fn open(&self) -> TestResult<Tunnel> {
        let until = Instant::now() + STEP;
        loop {
            let (status, tunnel) = self.connect_to().await?;
            if status.contains(" 200 ") {
                return Ok(tunnel);
            }
            assert!(status.contains(" 503 ") && Instant::now() < until, "{status}");
            sleep(Duration::from_millis(20)).await;
        }
    }

    fn connector(&self, version: &'static SupportedProtocolVersion) -> TlsConnector {
        TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder_with_protocol_versions(&[version])
                .with_root_certificates(self.roots.clone())
                .with_no_client_auth(),
        ))
    }

    /// A tunnel whose TLS session is up.
    async fn session(&self) -> TestResult<ClientTls> {
        let tunnel = self.open().await?;
        Ok(bounded(
            "the TLS handshake",
            self.connector(&rustls::version::TLS13)
                .connect(ServerName::try_from(HOST)?, tunnel.raw),
        )
        .await??)
    }

    /// One GET on `session`: the status of its answer (0: none came).
    async fn get(&self, session: &mut ClientTls, path: &str) -> TestResult<u16> {
        let request = format!("GET {path} HTTP/1.1\r\nHost: {}\r\n\r\n", self.target);
        bounded("sending a GET", session.write_all(request.as_bytes())).await??;
        status_of(session).await
    }

    /// One GET in a tunnel of its own.
    async fn send(&self, path: &str) -> TestResult<u16> {
        let mut session = self.session().await?;
        self.get(&mut session, path).await
    }

    /// The stalled tunnel holds the only slot until the deadline ends it.
    async fn expect_ended_at_deadline(&self, opened_at: Instant, closed: oneshot::Receiver<Instant>) -> TestResult {
        let (status, _refused) = self.connect_to().await?;
        assert!(status.contains(" 503 "), "{status}");
        let closed_at = timeout(4 * HANDSHAKE, closed)
            .await
            .map_err(|_| "the stalled tunnel was never closed")??;
        let after = closed_at - opened_at;
        assert!(after + EARLY >= HANDSHAKE, "closed after {after:?}");
        let _next = self.open().await?;
        Ok(())
    }
}

async fn read_head<S: AsyncReadExt + Unpin>(stream: &mut S) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte).await {
            Ok(1..) => head.push(byte[0]),
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

/// The status of the next answer on `session`, its body read off; 0 once
/// the session ends or nothing comes for [`STEP`].
async fn status_of(session: &mut ClientTls) -> TestResult<u16> {
    let Ok(Some(head)) = timeout(STEP, read_head(session)).await else {
        return Ok(0);
    };
    let length = match head
        .to_ascii_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:").map(str::to_owned))
    {
        Some(value) => value.trim().parse()?,
        None => 0,
    };
    let mut body = vec![0u8; length];
    if timeout(STEP, session.read_exact(&mut body)).await.is_err() {
        return Ok(0);
    }
    Ok(head
        .split(' ')
        .nth(1)
        .map_or(0, |status| status.parse().map_or(0, |code| code)))
}

/// Resets `raw` instead of closing it.
fn reset(raw: TcpStream) -> TestResult {
    raw.set_zero_linger()?;
    drop(raw);
    Ok(())
}

/// What came of a client whose writes to its tunnel stop once the server
/// has spoken, so that its Finished never arrives.
struct NoFinished {
    server_spoke: Arc<AtomicBool>,
    dropped: Arc<AtomicUsize>,
}

/// A real TLS client in front of `tunnel`, through a gate that lets its
/// ClientHello through and drops all it sends once the server has spoken.
fn no_finished(tunnel: Tunnel, connector: TlsConnector) -> TestResult<(NoFinished, oneshot::Receiver<Instant>)> {
    let server_name = ServerName::try_from(HOST)?;
    let (client_end, gate_end) = tokio::io::duplex(64 * 1024);
    let (mut from_client, mut to_client) = tokio::io::split(gate_end);
    let (mut raw_reads, mut raw_writes) = tunnel.raw.into_split();
    let server_spoke = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicUsize::new(0));
    let (closed, closed_at) = oneshot::channel();
    let spoke = Arc::clone(&server_spoke);
    tokio::spawn(async move {
        let mut chunk = [0u8; 16 * 1024];
        while let Ok(n @ 1..) = raw_reads.read(&mut chunk).await {
            spoke.store(true, Ordering::SeqCst);
            if to_client.write_all(&chunk[..n]).await.is_err() {
                break;
            }
        }
        let _ = closed.send(Instant::now());
    });
    let spoke = Arc::clone(&server_spoke);
    let held_back = Arc::clone(&dropped);
    tokio::spawn(async move {
        let mut chunk = [0u8; 16 * 1024];
        while let Ok(n @ 1..) = from_client.read(&mut chunk).await {
            if spoke.load(Ordering::SeqCst) {
                held_back.fetch_add(n, Ordering::SeqCst);
            } else if raw_writes.write_all(&chunk[..n]).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let _client = connector.connect(server_name, client_end).await;
        std::future::pending::<()>().await;
    });
    Ok((NoFinished { server_spoke, dropped }, closed_at))
}

async fn a_client_quiet_after_the_handshake_keeps_its_tunnel(version: &'static SupportedProtocolVersion) -> TestResult {
    let serve = Serve::start(HANDSHAKE).await?;
    let tunnel = serve.open().await?;
    let opened_at = tunnel.opened_at;
    let mut client = bounded(
        "the TLS handshake",
        serve
            .connector(version)
            .connect(ServerName::try_from(HOST)?, tunnel.raw),
    )
    .await??;
    assert_eq!(client.get_ref().1.protocol_version(), Some(version.version));
    // Past the handshake deadline, yet well inside the first-request one,
    // however long the handshake itself took.
    sleep_until(opened_at + HANDSHAKE + EARLY).await;
    let request = format!("GET / HTTP/1.1\r\nHost: {}\r\n\r\n", serve.target);
    bounded("sending a GET", client.write_all(request.as_bytes())).await??;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 1024];
    while !answer.windows(4).any(|window| window == b"\r\n\r\n") {
        match timeout(STEP, client.read(&mut chunk)).await {
            Ok(Ok(n @ 1..)) => answer.extend_from_slice(&chunk[..n]),
            _ => break,
        }
    }
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

async fn a_client_that_never_sends_its_finished_is_cut_off_and_its_slot_freed(
    version: &'static SupportedProtocolVersion,
) -> TestResult {
    let serve = Serve::start(HANDSHAKE).await?;
    let tunnel = serve.open().await?;
    let opened_at = tunnel.opened_at;
    let (client, closed) = no_finished(tunnel, serve.connector(version))?;
    serve.expect_ended_at_deadline(opened_at, closed).await?;
    assert!(client.server_spoke.load(Ordering::SeqCst));
    assert!(client.dropped.load(Ordering::SeqCst) > 0);
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn tls13_a_client_quiet_after_the_handshake_keeps_its_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    a_client_quiet_after_the_handshake_keeps_its_tunnel(&rustls::version::TLS13).await?;
    Ok(())
}

#[tokio::test]
async fn tls13_a_client_that_never_sends_its_finished_is_cut_off_and_its_slot_freed()
-> Result<(), Box<dyn std::error::Error>> {
    a_client_that_never_sends_its_finished_is_cut_off_and_its_slot_freed(&rustls::version::TLS13).await?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_that_got_going_outlives_the_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let handshake = 3 * HANDSHAKE;
    let serve = Serve::start(handshake).await?;
    let started = Instant::now();
    let mut session = serve.session().await?;
    assert_eq!(serve.get(&mut session, "/first").await?, 200);
    // Each answer restarts the keep-alive time, so the tunnel is kept busy
    // across the whole handshake deadline, whatever the load.
    while Instant::now() < started + handshake + EARLY {
        sleep(Duration::from_secs(1)).await;
        assert_eq!(serve.get(&mut session, "/kept").await?, 200);
    }
    assert_eq!(serve.get(&mut session, "/second").await?, 200);
    let seen = targets(&serve.seen)?;
    assert_eq!(seen.first().map(String::as_str), Some("/first"));
    assert_eq!(seen.last().map(String::as_str), Some("/second"));
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_reset_mid_handshake_and_a_reset_mid_request_end_only_their_tunnels() -> Result<(), Box<dyn std::error::Error>>
{
    let serve = Serve::start(UNTIMED).await?;

    let mut tunnel = serve.open().await?;
    let mut hello = Vec::new();
    rustls::ClientConnection::new(
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(serve.roots.clone())
                .with_no_client_auth(),
        ),
        ServerName::try_from(HOST)?,
    )?
    .write_tls(&mut hello)?;
    bounded("sending the ClientHello", tunnel.raw.write_all(&hello)).await??;
    let mut flight = [0u8; 1];
    bounded("the server's first flight", tunnel.raw.read_exact(&mut flight)).await??;
    reset(tunnel.raw)?;
    sleep(Duration::from_millis(200)).await;

    let mut session = serve.session().await?;
    let half = format!("GET /half HTTP/1.1\r\nHost: {}\r\n", serve.target);
    bounded("sending half a request", session.write_all(half.as_bytes())).await??;
    sleep(Duration::from_millis(50)).await;
    reset(session.into_inner().0)?;
    sleep(Duration::from_millis(200)).await;

    assert_eq!(serve.send("/ok").await?, 200);
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_client_dropped_mid_request_body_ends_only_its_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let serve = Serve::start(UNTIMED).await?;
    let mut session = serve.session().await?;
    session
        .write_all(
            format!(
                "POST /upload HTTP/1.1\r\nHost: {}\r\nContent-Length: 100000\r\n\r\npartial",
                serve.target
            )
            .as_bytes(),
        )
        .await?;
    sleep(Duration::from_millis(150)).await;
    drop(session);
    sleep(Duration::from_millis(200)).await;
    assert_eq!(serve.send("/ok").await?, 200);
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn garbage_after_the_handshake_ends_only_its_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let serve = Serve::start(UNTIMED).await?;
    let mut session = serve.session().await?;
    bounded("sending garbage", session.write_all(b"\x00\x01 not http\r\n\r\n")).await??;
    assert_eq!(status_of(&mut session).await?, 400);
    drop(session);
    sleep(Duration::from_millis(200)).await;
    assert_eq!(serve.send("/ok").await?, 200);
    assert!(targets(&serve.seen)?.iter().all(|target| target == "/ok"));
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[derive(Debug, PartialEq)]
enum End {
    Reset,
    Eof,
}

/// How the client's TCP connection ends after `first` and then `later` are
/// sent through the TLS session: with a reset, which is what closing a socket
/// that holds unread bytes does, or with the server's close.
async fn tunnel_end(serve: &Serve, first: &str, later: usize) -> TestResult<(End, u16)> {
    let session = serve.session().await?;
    let (mut reader, mut writer) = tokio::io::split(session);
    let mut everything = first.as_bytes().to_vec();
    everything.resize(everything.len() + later, b'a');
    let sender = tokio::spawn(async move {
        let _ = writer.write_all(&everything).await;
        writer
    });
    let mut status = 0;
    let mut head = Vec::new();
    let mut chunk = [0u8; 4096];
    let ended = loop {
        match timeout(Duration::from_secs(10), reader.read(&mut chunk)).await {
            Ok(Ok(0)) => break None,
            Ok(Ok(n)) => {
                head.extend_from_slice(&chunk[..n]);
                if status == 0 {
                    status = String::from_utf8_lossy(&head)
                        .split(' ')
                        .nth(1)
                        .map_or(0, |status| status.parse().map_or(0, |code| code));
                }
            }
            Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionReset => {
                break Some(End::Reset);
            }
            Ok(Err(_)) => break None,
            Err(_) => return Err("the tunnel neither ended nor answered".into()),
        }
    };
    let writer = sender.await?;
    if let Some(end) = ended {
        return Ok((end, status));
    }
    let mut session = reader.unsplit(writer);
    let raw = &mut session.get_mut().0;
    let end = loop {
        match timeout(Duration::from_secs(10), raw.read(&mut chunk)).await {
            Ok(Ok(0)) => break End::Eof,
            Ok(Ok(_)) => {}
            Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionReset => break End::Reset,
            other => return Err(format!("neither a reset nor an end: {other:?}").into()),
        }
    };
    Ok((end, status))
}

fn refused_in_tunnel(host: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "a body the proxy refuses",
            format!("POST /x HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: gzip\r\n\r\nabc"),
        ),
        (
            "a bad chunk",
            format!("POST /x HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"),
        ),
    ]
}

#[tokio::test]
async fn a_tunnel_ended_unanswered_with_unread_bytes_behind_it_resets() -> Result<(), Box<dyn std::error::Error>> {
    let serve = Serve::start(UNTIMED).await?;
    for (what, request) in refused_in_tunnel(&serve.target) {
        let (end, status) = tunnel_end(&serve, &request, 1 << 20).await?;

        assert_eq!(end, End::Reset, "{what}");
        assert_eq!(status, 0, "{what}");
    }
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_ended_unanswered_with_nothing_unread_ends_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    let serve = Serve::start(UNTIMED).await?;
    for (what, request) in refused_in_tunnel(&serve.target) {
        let (end, status) = tunnel_end(&serve, &request, 0).await?;

        assert_eq!(end, End::Eof, "{what}");
        assert_eq!(status, 0, "{what}");
    }
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_answered_for_a_parse_error_ends_cleanly_even_with_unread_bytes_behind_it()
-> Result<(), Box<dyn std::error::Error>> {
    let serve = Serve::start(UNTIMED).await?;
    let request = format!("TRACK /x HTTP/1.1\r\nHost: {}\r\n\r\n", serve.target);
    let (end, status) = tunnel_end(&serve, &request, 1 << 20).await?;

    assert_eq!(end, End::Eof);
    assert_eq!(status, 400);
    serve.server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}
