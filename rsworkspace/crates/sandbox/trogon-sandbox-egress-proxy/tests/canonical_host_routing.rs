#![cfg(unix)]

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener};
use trogon_sandbox_contract::{AllowRange, DomainRule, DomainRuleAction, LeaseId, NetworkPolicy, OpaqueTunnelPolicy};
use trogon_sandbox_contract::{AllowedTlsTermination, UnlistedDestinations};
use trogon_sandbox_contract::{DomainPattern, Port};
use trogon_sandbox_contract::{MitmUnixSocket, NetworkOptions, NoProxy, ParentProxy, ParentProxyUrl, TlsExclusions};
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const TOKEN: &str = "lease-token-value";

struct LoopbackLookup;

#[async_trait]
impl NameLookup for LoopbackLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))])
    }
}

struct NullSink;

#[async_trait]
impl ViolationSink for NullSink {
    async fn publish(&self, _: String, _: Vec<u8>) -> Result<(), ViolationPublishError> {
        Ok(())
    }
}

type Seen = Arc<Mutex<Vec<String>>>;

async fn read_head<S: AsyncReadExt + Unpin>(stream: &mut S) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn record(seen: &Seen, head: &str) {
    let mut lines = head.split("\r\n");
    let first = lines.next().map_or("", |line| line);
    let mut parts = first.split(' ');
    let method = parts.next().map_or("", |part| part);
    let target = parts.next().map_or("", |part| part);
    let host = lines
        .find_map(|line| line.strip_prefix("host: "))
        .map_or("", |value| value);
    let entry = if method == "CONNECT" {
        target.to_string()
    } else {
        format!("{method} {target} host={host}")
    };
    if let Ok(mut list) = seen.lock() {
        list.push(entry);
    }
}

async fn answer<S: AsyncReadExt + AsyncWriteExt + Unpin>(mut stream: S, seen: Seen) {
    let Ok(head) = read_head(&mut stream).await else {
        return;
    };
    record(&seen, &head);
    if head.starts_with("CONNECT ") {
        let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
        let mut sink = [0u8; 64];
        while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
    } else {
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nvia")
            .await;
    }
}

struct Mitm {
    socket: PathBuf,
    seen: Seen,
    _dir: tempfile::TempDir,
}

async fn mitm_listening() -> TestResult<Mitm> {
    let dir = tempfile::Builder::new().prefix("chr").tempdir()?;
    let socket = dir.path().join("m.sock");
    let listener = UnixListener::bind(&socket)?;
    let seen = Seen::default();
    let recorded = seen.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(answer(stream, recorded.clone()));
        }
    });
    Ok(Mitm {
        socket,
        seen,
        _dir: dir,
    })
}

fn mitm_without_listener() -> TestResult<Mitm> {
    let dir = tempfile::Builder::new().prefix("chr").tempdir()?;
    Ok(Mitm {
        socket: dir.path().join("none.sock"),
        seen: Seen::default(),
        _dir: dir,
    })
}

struct Parent {
    port: u16,
    seen: Seen,
}

async fn parent_listening() -> TestResult<Parent> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let seen = Seen::default();
    let recorded = seen.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(answer(stream, recorded.clone()));
        }
    });
    Ok(Parent { port, seen })
}

struct Origin {
    port: u16,
    connections: Arc<AtomicUsize>,
}

async fn origin_listening() -> TestResult<Origin> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = connections.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut sink = [0u8; 256];
                while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
    });
    Ok(Origin { port, connections })
}

#[derive(Default)]
struct Setup {
    allowed: Vec<&'static str>,
    denied: Vec<&'static str>,
    mitm_domains: Vec<&'static str>,
    exclusions: Vec<&'static str>,
    loopback: bool,
    refuse_opaque: bool,
}

struct Harness {
    server: ProxyServer,
    addr: SocketAddr,
}

async fn start(setup: Setup, mitm: Option<&Mitm>, parent: Option<&Parent>, terminate: bool) -> TestResult<Harness> {
    install_default_crypto_provider();
    let policy = NetworkPolicy::UserDefined {
        rules: setup
            .allowed
            .iter()
            .map(|domain| {
                Ok(DomainRule {
                    domain: domain.parse()?,
                    action: DomainRuleAction::PassThrough,
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
        allow_ranges: if setup.loopback {
            vec![AllowRange::new("127.0.0.0/8")?]
        } else {
            vec![]
        },
        deny_ranges: vec![],
        denied_domains: setup.denied.iter().map(|d| d.parse()).collect::<Result<Vec<_>, _>>()?,
        opaque_tunnels: if setup.refuse_opaque {
            OpaqueTunnelPolicy::Refuse
        } else {
            OpaqueTunnelPolicy::Allow
        },
        request_rules: vec![],
    };
    let mut options = NetworkOptions {
        unlisted: UnlistedDestinations::Refuse,
        ..NetworkOptions::default()
    };
    if let Some(mitm) = mitm {
        options.mitm = Some(MitmUnixSocket {
            socket_path: mitm.socket.clone(),
            domains: setup
                .mitm_domains
                .iter()
                .map(|d| DomainPattern::new(d))
                .collect::<Result<Vec<_>, _>>()?,
        });
    }
    if let Some(parent) = parent {
        let url = ParentProxyUrl::new(&format!("http://127.0.0.1:{}", parent.port))?;
        options.parent_proxy = Some(ParentProxy::new(Some(url), None, NoProxy::new(""))?);
    }
    if terminate {
        options.terminate_allowed_tls = AllowedTlsTermination::Enabled;
    }
    if !setup.exclusions.is_empty() {
        options.tls_exclusions = TlsExclusions::new(
            setup
                .exclusions
                .iter()
                .map(|d| DomainPattern::new(d))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    let ca = SandboxCa::generate(PermittedDomains::from_rules(&[])?)?.permitting_every_name();
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy, ca)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .violations(Arc::new(ViolationReporter::new(
            LeaseId::new("L1"),
            Arc::new(NullSink),
            ViolationRateLimit::default(),
        )))
        .resolver(Arc::new(VettingResolver::new(
            Arc::new(LoopbackLookup),
            SsrfFloor::system(),
        )))
        .network_options(options)
        .start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("{other:?}").into()),
    };
    Ok(Harness { server, addr })
}

impl Harness {
    async fn finish(self) -> TestResult {
        self.server.shutdown(Duration::from_millis(100)).await?;
        Ok(())
    }

    async fn connect_status(&self, target: &str) -> TestResult<String> {
        let mut stream = TcpStream::connect(self.addr).await?;
        let auth = format!("Basic {}", STANDARD.encode(format!("sandbox:{TOKEN}")));
        stream
            .write_all(
                format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {auth}\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        while !reply.ends_with(b"\r\n") {
            if !matches!(stream.read(&mut byte).await, Ok(1..)) {
                break;
            }
            reply.push(byte[0]);
        }
        Ok(String::from_utf8_lossy(&reply).trim_end().to_string())
    }

    async fn get(&self, absolute: &str, host: &str) -> TestResult<String> {
        let mut stream = TcpStream::connect(self.addr).await?;
        let auth = format!("Basic {}", STANDARD.encode(format!("sandbox:{TOKEN}")));
        stream
            .write_all(
                format!(
                    "GET {absolute} HTTP/1.1\r\nHost: {host}\r\nProxy-Authorization: {auth}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let mut reply = Vec::new();
        let _ = stream.read_to_end(&mut reply).await;
        Ok(String::from_utf8_lossy(&reply).into_owned())
    }

    async fn socks(&self, host: &str, port: u16) -> TestResult<u8> {
        let mut stream = TcpStream::connect(self.addr).await?;
        stream.write_all(&[5, 1, 2]).await?;
        let mut method = [0u8; 2];
        stream.read_exact(&mut method).await?;
        assert_eq!(method, [5, 2]);
        let user = b"sandbox";
        let mut auth = vec![1, user.len() as u8];
        auth.extend_from_slice(user);
        auth.push(TOKEN.len() as u8);
        auth.extend_from_slice(TOKEN.as_bytes());
        stream.write_all(&auth).await?;
        let mut status = [0u8; 2];
        stream.read_exact(&mut status).await?;
        assert_eq!(status, [1, 0]);
        let mut request = vec![5, 1, 0, 3, host.len() as u8];
        request.extend_from_slice(host.as_bytes());
        request.extend(port.to_be_bytes());
        stream.write_all(&request).await?;
        let mut reply = [0u8; 10];
        stream.read_exact(&mut reply).await?;
        Ok(reply[1])
    }
}

fn seen(list: &Seen) -> TestResult<Vec<String>> {
    Ok(list.lock().map_err(|_| "seen lock poisoned")?.clone())
}

fn is_200(status: &str) -> bool {
    status.starts_with("HTTP/1.1 200 ")
}

fn is_403(status: &str) -> bool {
    status.starts_with("HTTP/1.1 403 ")
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn connect_spellings_of_a_mitm_name_reach_the_mitm_socket_canonically() -> TestResult {
    for pattern in ["api.mitm.invalid", "*.mitm.invalid"] {
        let mitm = mitm_listening().await?;
        let h = start(
            Setup {
                allowed: vec![pattern],
                mitm_domains: vec![pattern],
                ..Default::default()
            },
            Some(&mitm),
            None,
            false,
        )
        .await?;
        for target in ["Api.MITM.invalid.:443", "api.mitm.invalid.:443", "API.Mitm.Invalid:443"] {
            assert!(is_200(&h.connect_status(target).await?), "{pattern} {target}");
        }
        assert_eq!(seen(&mitm.seen)?, vec!["api.mitm.invalid:443"; 3], "{pattern}");
        h.finish().await?;
    }
    Ok(())
}

#[tokio::test]
async fn connect_inet_aton_spellings_of_a_mitm_pinned_ip_reach_the_mitm_socket() -> TestResult {
    let mitm = mitm_listening().await?;
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["127.0.0.1"],
            mitm_domains: vec!["127.0.0.1"],
            loopback: true,
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    for spelling in ["127.1", "0x7f.0.0.1", "2130706433", "127.0.0.1."] {
        let target = format!("{spelling}:{}", origin.port);
        assert!(is_200(&h.connect_status(&target).await?), "{spelling}");
    }
    assert_eq!(seen(&mitm.seen)?, vec![format!("127.0.0.1:{}", origin.port); 4]);
    settle().await;
    assert_eq!(origin.connections.load(Ordering::SeqCst), 0);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn connect_to_an_allowed_non_mitm_spelling_dials_directly() -> TestResult {
    let mitm = mitm_listening().await?;
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid", "127.0.0.1"],
            mitm_domains: vec!["api.mitm.invalid"],
            loopback: true,
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    for spelling in ["127.1", "127.0.0.1."] {
        let target = format!("{spelling}:{}", origin.port);
        assert!(is_200(&h.connect_status(&target).await?), "{spelling}");
    }
    settle().await;
    assert_eq!(origin.connections.load(Ordering::SeqCst), 2);
    assert!(seen(&mitm.seen)?.is_empty());
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn connect_to_a_host_outside_the_allowlist_never_reaches_the_mitm() -> TestResult {
    let mitm = mitm_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid"],
            mitm_domains: vec!["*.mitm.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    for target in [
        "other.mitm.invalid:443",
        "other.mitm.invalid.:443",
        "OTHER.mitm.invalid.:443",
    ] {
        assert!(is_403(&h.connect_status(target).await?), "{target}");
    }
    assert!(seen(&mitm.seen)?.is_empty());
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn an_ipv4_mapped_spelling_of_a_denied_address_is_refused_though_allowed() -> TestResult {
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["[::ffff:127.0.0.1]"],
            denied: vec!["127.0.0.1"],
            ..Default::default()
        },
        None,
        None,
        false,
    )
    .await?;
    for host in ["[::ffff:127.0.0.1]", "[::FFFF:7F00:1]", "[0:0:0:0:0:ffff:7f00:1]"] {
        let target = format!("{host}:{}", origin.port);
        assert!(is_403(&h.connect_status(&target).await?), "{host}");
    }
    settle().await;
    assert_eq!(origin.connections.load(Ordering::SeqCst), 0);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn denied_domains_win_over_every_spelling_and_a_sibling_still_routes() -> TestResult {
    let mitm = mitm_listening().await?;
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["*.mitm.invalid", "127.0.0.1"],
            denied: vec!["evil.mitm.invalid", "127.0.0.1"],
            mitm_domains: vec!["*.mitm.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    let port = origin.port;
    for target in [
        "evil.mitm.invalid:443".to_string(),
        "evil.mitm.invalid.:443".to_string(),
        "EVIL.MITM.invalid.:443".to_string(),
        format!("127.1:{port}"),
        format!("0x7f.0.0.1:{port}"),
        format!("2130706433:{port}"),
        format!("0177.0.0.1:{port}"),
    ] {
        assert!(is_403(&h.connect_status(&target).await?), "{target}");
    }
    let status = h.connect_status("ok.mitm.invalid.:443").await?;
    assert!(is_200(&status), "{status}");
    assert_eq!(seen(&mitm.seen)?, vec!["ok.mitm.invalid:443"]);
    settle().await;
    assert_eq!(origin.connections.load(Ordering::SeqCst), 0);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn connect_via_a_parent_proxy_mitm_wins_and_the_parent_gets_the_canonical_authority() -> TestResult {
    let mitm = mitm_listening().await?;
    let parent = parent_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid", "other.example.invalid"],
            mitm_domains: vec!["api.mitm.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        Some(&parent),
        false,
    )
    .await?;
    assert!(is_200(&h.connect_status("api.mitm.invalid.:443").await?));
    assert!(is_200(&h.connect_status("Other.Example.invalid.:443").await?));
    assert_eq!(seen(&mitm.seen)?, vec!["api.mitm.invalid:443"]);
    assert_eq!(seen(&parent.seen)?, vec!["other.example.invalid:443"]);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn the_tls_exemption_is_decided_on_the_canonical_host() -> TestResult {
    let mitm = mitm_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["pinned.invalid"],
            mitm_domains: vec!["pinned.invalid"],
            exclusions: vec!["pinned.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        None,
        true,
    )
    .await?;
    assert!(is_200(&h.connect_status("Pinned.invalid.:443").await?));
    assert_eq!(seen(&mitm.seen)?, vec!["pinned.invalid:443"]);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_is_forwarded_under_the_canonical_authority() -> TestResult {
    let parent = parent_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["files.example.invalid", "api.mitm.invalid", "[2001:db8::1]"],
            ..Default::default()
        },
        None,
        Some(&parent),
        false,
    )
    .await?;
    let reply = h
        .get("http://Files.Example.invalid./v1/files", "Files.Example.invalid")
        .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let reply = h
        .get("http://Api.MITM.invalid.:8080/x?q=1", "Api.MITM.invalid:8080")
        .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let reply = h
        .get(
            "http://[2001:0db8:0000:0000:0000:0000:0000:0001]:8080/x",
            "[2001:db8::1]:8080",
        )
        .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let denied = h
        .get("http://other.example.invalid./v1/files", "other.example.invalid")
        .await?;
    assert!(denied.starts_with("HTTP/1.1 403 "), "{denied}");
    assert_eq!(
        seen(&parent.seen)?,
        vec![
            "GET http://files.example.invalid/v1/files host=files.example.invalid",
            "GET http://api.mitm.invalid:8080/x?q=1 host=api.mitm.invalid:8080",
            "GET http://[2001:db8::1]:8080/x host=[2001:db8::1]:8080",
        ]
    );
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn socks_spellings_of_a_mitm_name_reach_the_mitm_socket() -> TestResult {
    for pattern in ["api.mitm.invalid", "*.mitm.invalid"] {
        let mitm = mitm_listening().await?;
        let parent = parent_listening().await?;
        let h = start(
            Setup {
                allowed: vec![pattern],
                mitm_domains: vec![pattern],
                ..Default::default()
            },
            Some(&mitm),
            Some(&parent),
            false,
        )
        .await?;
        for spelling in ["api.mitm.invalid", "api.mitm.invalid.", "API.Mitm.Invalid"] {
            assert_eq!(h.socks(spelling, 443).await?, 0, "{pattern} {spelling}");
        }
        assert_eq!(seen(&mitm.seen)?, vec!["api.mitm.invalid:443"; 3]);
        assert!(seen(&parent.seen)?.is_empty());
        h.finish().await?;
    }
    Ok(())
}

#[tokio::test]
async fn socks_to_a_mitm_pinned_ip_never_dials_the_address() -> TestResult {
    let mitm = mitm_listening().await?;
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["127.0.0.1"],
            mitm_domains: vec!["127.0.0.1"],
            loopback: true,
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    for spelling in ["127.0.0.1", "127.1", "127.0.0.1."] {
        assert_eq!(h.socks(spelling, origin.port).await?, 0, "{spelling}");
    }
    assert_eq!(seen(&mitm.seen)?, vec![format!("127.0.0.1:{}", origin.port); 3]);
    settle().await;
    assert_eq!(origin.connections.load(Ordering::SeqCst), 0);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn socks_via_a_parent_proxy_mitm_wins_and_the_parent_gets_the_canonical_authority() -> TestResult {
    let mitm = mitm_listening().await?;
    let parent = parent_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid", "other.example.invalid"],
            mitm_domains: vec!["api.mitm.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        Some(&parent),
        false,
    )
    .await?;
    for host in ["api.mitm.invalid.", "Other.Example.invalid."] {
        assert_eq!(h.socks(host, 443).await?, 0, "{host}");
    }
    assert_eq!(seen(&mitm.seen)?, vec!["api.mitm.invalid:443"]);
    assert_eq!(seen(&parent.seen)?, vec!["other.example.invalid:443"]);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn socks_to_an_allowed_host_outside_the_mitm_domains_dials_directly() -> TestResult {
    let mitm = mitm_listening().await?;
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid", "127.0.0.1"],
            mitm_domains: vec!["api.mitm.invalid"],
            loopback: true,
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    assert_eq!(h.socks("127.0.0.1", origin.port).await?, 0);
    settle().await;
    assert_eq!(origin.connections.load(Ordering::SeqCst), 1);
    assert!(seen(&mitm.seen)?.is_empty());
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn socks_with_an_unreachable_mitm_socket_is_host_unreachable_without_a_fallback() -> TestResult {
    let mitm = mitm_without_listener()?;
    let parent = parent_listening().await?;
    let origin = origin_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid", "127.0.0.1"],
            mitm_domains: vec!["api.mitm.invalid", "127.0.0.1"],
            loopback: true,
            ..Default::default()
        },
        Some(&mitm),
        Some(&parent),
        false,
    )
    .await?;
    assert_eq!(h.socks("api.mitm.invalid", 443).await?, 4);
    assert_eq!(h.socks("127.0.0.1", origin.port).await?, 4);
    settle().await;
    assert!(seen(&parent.seen)?.is_empty());
    assert_eq!(origin.connections.load(Ordering::SeqCst), 0);
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn socks_to_a_mitm_domain_outside_the_allowlist_is_refused_untouched() -> TestResult {
    let mitm = mitm_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid"],
            mitm_domains: vec!["*.mitm.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    for host in ["other.mitm.invalid", "OTHER.mitm.invalid."] {
        assert_eq!(h.socks(host, 443).await?, 2, "{host}");
    }
    assert!(seen(&mitm.seen)?.is_empty());
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_to_a_mitm_name_is_sent_to_the_socket_in_absolute_canonical_form() -> TestResult {
    let mitm = mitm_listening().await?;
    let parent = parent_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid"],
            mitm_domains: vec!["api.mitm.invalid"],
            ..Default::default()
        },
        Some(&mitm),
        Some(&parent),
        false,
    )
    .await?;
    let reply = h
        .get("http://Api.MITM.invalid./v1/thing?q=1", "Api.MITM.invalid")
        .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    assert_eq!(
        seen(&mitm.seen)?,
        vec!["GET http://api.mitm.invalid/v1/thing?q=1 host=api.mitm.invalid"]
    );
    assert!(seen(&parent.seen)?.is_empty());
    h.finish().await?;
    Ok(())
}

#[tokio::test]
async fn socks_to_a_mitm_host_is_refused_without_asking_for_the_socket_when_opaque_tunnels_are_refused() -> TestResult {
    let mitm = mitm_listening().await?;
    let h = start(
        Setup {
            allowed: vec!["api.mitm.invalid"],
            mitm_domains: vec!["api.mitm.invalid"],
            refuse_opaque: true,
            ..Default::default()
        },
        Some(&mitm),
        None,
        false,
    )
    .await?;
    assert_eq!(h.socks("api.mitm.invalid", 443).await?, 1);
    settle().await;
    assert!(seen(&mitm.seen)?.is_empty());
    h.finish().await?;
    Ok(())
}
