//! How a terminated tunnel's upstream leg reaches its target, as the upstream
//! sees it: a name is vetted once per leg with a probe connection that is
//! closed before anything is sent, then the address that answered is dialed
//! for every request on the tunnel, a redial after the upstream closed an idle
//! connection included.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};
use tokio_rustls::TlsConnector;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{AllowRange, DomainRule, DomainRuleAction, LeaseId, NetworkPolicy};
use trogon_sandbox_contract::{AllowedTlsTermination, NetworkOptions, UpstreamCaCertificates};
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer};
use trogon_sandbox_egress_proxy::tls_ca::{
    PermittedDomains, SandboxCa, install_default_crypto_provider, server_config_for_leaf,
};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const TOKEN: &str = "lease-token-value";
const HOST: &str = "allowed.test";
const WAIT: Duration = Duration::from_secs(5);

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

/// One connection the upstream accepted: a probe sends nothing before it
/// closes, any other either fails its handshake or carries the paths of the
/// requests it served.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Accepted {
    Pending,
    Probe,
    Unverified,
    Served(Vec<String>),
}

fn record(recorded: &Mutex<Vec<Accepted>>, index: usize, value: Accepted) {
    if let Ok(mut accepted) = recorded.lock()
        && let Some(slot) = accepted.get_mut(index)
    {
        *slot = value;
    }
}

type ClientTls = tokio_rustls::client::TlsStream<TcpStream>;

struct Serve {
    _server: ProxyServer,
    proxy: SocketAddr,
    port: u16,
    roots: rustls::RootCertStore,
    accepted: Arc<Mutex<Vec<Accepted>>>,
}

impl Serve {
    /// An HTTPS upstream on loopback, certified for `certified`, that closes
    /// a connection once it has answered `/bye` and drops one unanswered on
    /// `/drop`, behind a proxy that terminates every allowed tunnel.
    async fn start(certified: &str) -> TestResult<Self> {
        install_default_crypto_provider();
        let rules = vec![DomainRule {
            domain: HOST.parse()?,
            action: DomainRuleAction::PassThrough,
        }];
        let certified_rules = vec![DomainRule {
            domain: certified.parse()?,
            action: DomainRuleAction::PassThrough,
        }];
        let upstream_ca = SandboxCa::generate(PermittedDomains::from_rules(&certified_rules)?)?;
        let acceptor = tokio_rustls::TlsAcceptor::from(server_config_for_leaf(upstream_ca.mint_leaf_cert(certified)?));
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let port = upstream.local_addr()?.port();
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok((stream, _)) = upstream.accept().await {
                let acceptor = acceptor.clone();
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let index = {
                        let Ok(mut accepted) = recorded.lock() else {
                            return;
                        };
                        accepted.push(Accepted::Pending);
                        accepted.len() - 1
                    };
                    if let Ok(0) = stream.peek(&mut [0u8; 1]).await {
                        record(&recorded, index, Accepted::Probe);
                        return;
                    }
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        record(&recorded, index, Accepted::Unverified);
                        return;
                    };
                    record(&recorded, index, Accepted::Served(Vec::new()));
                    while let Some(head) = read_head(&mut tls).await {
                        let path = head.split(' ').nth(1).map_or("", |part| part).to_owned();
                        if let Ok(mut accepted) = recorded.lock()
                            && let Some(Accepted::Served(paths)) = accepted.get_mut(index)
                        {
                            paths.push(path.clone());
                        }
                        if path == "/drop" {
                            return;
                        }
                        if tls
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .is_err()
                            || path == "/bye"
                        {
                            return;
                        }
                    }
                });
            }
        });
        let ca = SandboxCa::generate(PermittedDomains::from_rules(&rules)?)?.permitting_every_name();
        let policy = NetworkPolicy::UserDefined {
            rules,
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
            .start()?;
        let proxy = match server.explicit_addr() {
            Some(BoundAddress::Tcp(addr)) => *addr,
            other => return Err(format!("{other:?}").into()),
        };
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut io::Cursor::new(server.ca_certificate_pem().as_bytes().to_vec())) {
            roots.add(cert?)?;
        }
        Ok(Self {
            _server: server,
            proxy,
            port,
            roots,
            accepted,
        })
    }

    /// A tunnel to `host` on the upstream's port, its TLS session up under
    /// the allowed name.
    async fn session(&self, host: &str) -> TestResult<ClientTls> {
        let target = format!("{host}:{}", self.port);
        let mut raw = TcpStream::connect(self.proxy).await?;
        let authorization = STANDARD.encode(format!("sandbox:{TOKEN}"));
        raw.write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {authorization}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
        let head = read_head(&mut raw).await.ok_or("the proxy closed before answering")?;
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        Ok(TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(self.roots.clone())
                .with_no_client_auth(),
        ))
        .connect(ServerName::try_from(host.to_owned())?, raw)
        .await?)
    }

    /// One GET on `session`: the status of its answer, its body read off.
    async fn get(&self, session: &mut ClientTls, path: &str) -> TestResult<u16> {
        session
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: {HOST}:{}\r\n\r\n", self.port).as_bytes())
            .await?;
        let head = timeout(WAIT, read_head(session))
            .await
            .map_err(|_| "no answer came")?
            .ok_or("the tunnel ended")?;
        let lowered = head.to_ascii_lowercase();
        if lowered.contains("\r\ntransfer-encoding: chunked\r\n") {
            let mut body = Vec::new();
            while !body.ends_with(b"\r\n0\r\n\r\n") {
                body.push(session.read_u8().await?);
            }
        } else {
            let length = match lowered
                .lines()
                .find_map(|line| line.strip_prefix("content-length:").map(str::to_owned))
            {
                Some(value) => value.trim().parse()?,
                None => 0,
            };
            let mut body = vec![0u8; length];
            session.read_exact(&mut body).await?;
        }
        Ok(head.split(' ').nth(1).ok_or("no status in the answer")?.parse()?)
    }

    /// What a GET on `session` brings before the tunnel ends, and how it
    /// ends.
    async fn ended_by(&self, session: &mut ClientTls, path: &str) -> TestResult<(Vec<u8>, io::ErrorKind)> {
        session
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: {HOST}:{}\r\n\r\n", self.port).as_bytes())
            .await?;
        let mut seen = Vec::new();
        let ended = timeout(WAIT, session.read_to_end(&mut seen))
            .await
            .map_err(|_| "the tunnel stayed open")?;
        Ok((seen, ended.map_or_else(|error| error.kind(), |_| io::ErrorKind::Other)))
    }

    /// What the upstream accepted, once every connection has shown what it is.
    async fn accepted(&self, count: usize) -> TestResult<Vec<Accepted>> {
        let snapshot = || {
            self.accepted
                .lock()
                .map(|accepted| accepted.clone())
                .map_err(|_| "the record is poisoned")
        };
        let settled = timeout(WAIT, async {
            loop {
                let accepted = snapshot()?;
                if accepted.len() >= count && !accepted.contains(&Accepted::Pending) {
                    return Ok::<_, &str>(accepted);
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        match settled {
            Ok(accepted) => Ok(accepted?),
            Err(_) => Ok(snapshot()?),
        }
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

fn served(paths: &[&str]) -> Accepted {
    Accepted::Served(paths.iter().map(|path| (*path).to_owned()).collect())
}

#[tokio::test]
async fn a_named_leg_is_vetted_by_one_probe_and_redials_without_another() -> TestResult {
    let serve = Serve::start(HOST).await?;
    let mut first = serve.session(HOST).await?;
    assert_eq!(serve.get(&mut first, "/a").await?, 200);
    assert_eq!(serve.get(&mut first, "/bye").await?, 200);
    sleep(Duration::from_millis(100)).await;
    assert_eq!(serve.get(&mut first, "/b").await?, 200);
    let mut second = serve.session(HOST).await?;
    assert_eq!(serve.get(&mut second, "/c").await?, 200);
    assert_eq!(
        serve.accepted(5).await?,
        [
            Accepted::Probe,
            served(&["/a", "/bye"]),
            served(&["/b"]),
            Accepted::Probe,
            served(&["/c"]),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_literal_leg_is_dialed_as_given_without_a_probe() -> TestResult {
    let serve = Serve::start(HOST).await?;
    let mut session = serve.session("127.0.0.1").await?;
    for path in ["/a", "/b"] {
        session
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", serve.port).as_bytes())
            .await?;
        timeout(WAIT, read_head(&mut session))
            .await
            .map_err(|_| "no answer came")?
            .ok_or("the tunnel ended")?;
    }
    assert_eq!(serve.accepted(2).await?, [Accepted::Unverified, Accepted::Unverified]);
    Ok(())
}

#[tokio::test]
async fn a_failed_upstream_dial_forgets_the_vetted_address() -> TestResult {
    let serve = Serve::start("other.test").await?;
    let mut session = serve.session(HOST).await?;
    assert_eq!(serve.get(&mut session, "/a").await?, 502);
    assert_eq!(serve.get(&mut session, "/b").await?, 502);
    assert_eq!(
        serve.accepted(4).await?,
        [
            Accepted::Probe,
            Accepted::Unverified,
            Accepted::Probe,
            Accepted::Unverified,
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_kept_alive_upstream_dropping_a_request_closes_the_client_connection_unanswered() -> TestResult {
    let serve = Serve::start(HOST).await?;
    let mut session = serve.session(HOST).await?;
    assert_eq!(serve.get(&mut session, "/a").await?, 200);
    let (seen, ended) = serve.ended_by(&mut session, "/drop").await?;
    assert_eq!(String::from_utf8_lossy(&seen), "");
    assert_eq!(ended, io::ErrorKind::UnexpectedEof, "no close_notify");
    Ok(())
}

#[tokio::test]
async fn a_fresh_upstream_dropping_a_request_is_answered_502_and_the_tunnel_stays_open() -> TestResult {
    let serve = Serve::start(HOST).await?;
    let mut session = serve.session(HOST).await?;
    assert_eq!(serve.get(&mut session, "/drop").await?, 502);
    assert_eq!(serve.get(&mut session, "/a").await?, 200);
    Ok(())
}
