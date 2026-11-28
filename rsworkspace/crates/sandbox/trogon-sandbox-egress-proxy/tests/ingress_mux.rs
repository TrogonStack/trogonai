use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
#[cfg(unix)]
use trogon_sandbox_contract::DomainPattern;
#[cfg(unix)]
use trogon_sandbox_contract::MitmUnixSocket;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{
    AllowRange, DomainRule, DomainRuleAction, EgressViolation, EgressViolationReason, HttpMethod, IpRange, LeaseId,
    NetworkPolicy,
};
use trogon_sandbox_contract::{HeaderCondition, HeaderName, HeaderValue, PathPrefix, RequestRule, RequestRuleAction};
use trogon_sandbox_contract::{NetworkOptions, NoProxy, ParentProxy, ParentProxyUrl, ReasonedRefusals, TunnelCap};
use trogon_sandbox_contract::{ResponseHeaderStrip, StrippedHeader};
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;
use trogon_sandbox_egress_proxy::ingress::{Ingress, IngressLimits, IngressOutcome, IngressRejectionError, ProxyToken};
use trogon_sandbox_egress_proxy::oidc_signer::OidcSigner;
use trogon_sandbox_egress_proxy::proxy::{OidcIdentityContext, ProxyOutcome, TlsInterceptContext};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};

const TOKEN: &str = "lease-token-value";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

trait OptionsExt: Sized {
    fn with_reasoned_refusals(self) -> Self;
    fn with_parent_proxy(self, parent_proxy: ParentProxy) -> Self;
    #[cfg(unix)]
    fn with_mitm(self, mitm: MitmUnixSocket) -> Self;
    fn with_tunnel_cap(self, cap: TunnelCap) -> Self;
    fn with_strip_response_headers(self, strip: ResponseHeaderStrip) -> Self;
}

impl OptionsExt for NetworkOptions {
    fn with_reasoned_refusals(mut self) -> Self {
        self.reasoned_refusals = ReasonedRefusals::Enabled;
        self
    }

    fn with_parent_proxy(mut self, parent_proxy: ParentProxy) -> Self {
        self.parent_proxy = Some(parent_proxy);
        self
    }

    #[cfg(unix)]
    fn with_mitm(mut self, mitm: MitmUnixSocket) -> Self {
        self.mitm = Some(mitm);
        self
    }

    fn with_tunnel_cap(mut self, cap: TunnelCap) -> Self {
        self.tunnel_cap = Some(cap);
        self
    }

    fn with_strip_response_headers(mut self, strip: ResponseHeaderStrip) -> Self {
        self.strip_response_headers = strip;
        self
    }
}

fn lock<T>(mutex: &Mutex<T>) -> TestResult<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| "the mutex was poisoned".into())
}

struct StubLookup {
    calls: AtomicUsize,
    answers: Vec<IpAddr>,
    delay: Duration,
    held: Option<Arc<Semaphore>>,
}

#[async_trait]
impl NameLookup for StubLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(held) = &self.held {
            held.acquire().await.map_err(io::Error::other)?.forget();
        }
        tokio::time::sleep(self.delay).await;
        Ok(self.answers.clone())
    }
}

#[derive(Default)]
struct CollectingSink(Mutex<Vec<Vec<u8>>>);

#[async_trait]
impl ViolationSink for CollectingSink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        lock(&self.0)
            .map_err(|error| ViolationPublishError(error.to_string()))?
            .push(payload);
        Ok(())
    }
}

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: "allowed.test".parse()?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![AllowRange::new("127.0.0.0/8")?],
        deny_ranges: vec![IpRange::new("10.0.0.0/8")?],
        denied_domains: vec!["blocked.test".parse()?],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    })
}

fn tls_context() -> TestResult<TlsInterceptContext> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(PermittedDomains::from_rules(&[])?)?;
    Ok(TlsInterceptContext::new(
        ca,
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        ),
        Arc::new(UnconfiguredCredentialSource),
        OidcIdentityContext::unconfigured(OidcSigner::generate()?, "test")?,
    ))
}

struct Harness {
    addr: SocketAddr,
    outcomes: mpsc::UnboundedReceiver<IngressOutcome>,
    resolver: Arc<StubLookup>,
    sink: Arc<CollectingSink>,
}

async fn start(limits: IngressLimits) -> TestResult<Harness> {
    start_answering(limits, IpAddr::V4(Ipv4Addr::LOCALHOST)).await
}

async fn start_answering(limits: IngressLimits, answer: IpAddr) -> TestResult<Harness> {
    start_with(limits, answer, policy()?).await
}

async fn start_with(limits: IngressLimits, answer: IpAddr, network: NetworkPolicy) -> TestResult<Harness> {
    start_resolving(limits, vec![answer], network).await
}

async fn start_resolving(limits: IngressLimits, answers: Vec<IpAddr>, network: NetworkPolicy) -> TestResult<Harness> {
    start_deciding(limits, answers, network, Duration::ZERO, false).await
}

async fn start_deciding(
    limits: IngressLimits,
    answers: Vec<IpAddr>,
    network: NetworkPolicy,
    decision_time: Duration,
    capture: bool,
) -> TestResult<Harness> {
    start_holding(limits, answers, network, decision_time, capture, None).await
}

async fn start_holding(
    limits: IngressLimits,
    answers: Vec<IpAddr>,
    network: NetworkPolicy,
    decision_time: Duration,
    capture: bool,
    held: Option<Arc<Semaphore>>,
) -> TestResult<Harness> {
    start_configured(
        limits,
        answers,
        network,
        decision_time,
        capture,
        held,
        NetworkOptions::default(),
    )
    .await
}

async fn start_with_options(options: NetworkOptions) -> TestResult<Harness> {
    start_configured(
        IngressLimits::default(),
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        policy()?,
        Duration::ZERO,
        false,
        None,
        options,
    )
    .await
}

async fn start_configured(
    limits: IngressLimits,
    answers: Vec<IpAddr>,
    network: NetworkPolicy,
    decision_time: Duration,
    capture: bool,
    held: Option<Arc<Semaphore>>,
    options: NetworkOptions,
) -> TestResult<Harness> {
    let network = Arc::new(network);
    let options = Arc::new(options);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let resolver = Arc::new(StubLookup {
        calls: AtomicUsize::new(0),
        answers,
        delay: decision_time,
        held,
    });
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink.clone(),
        ViolationRateLimit::default(),
    ));
    let ingress = Arc::new(
        Ingress::new(
            ProxyToken::new(TOKEN)?,
            Arc::new(VettingResolver::new(resolver.clone(), SsrfFloor::system())),
            limits,
        )
        .with_network_options(options.clone()),
    );
    let mut tls = tls_context()?.with_decision_window_capture(capture);
    tls.network_options = options;
    let tls = Arc::new(tls);
    let (tx, outcomes) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let ingress = ingress.clone();
            let tls = tls.clone();
            let network = network.clone();
            let reporter = reporter.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let outcome = ingress.serve(stream, &network, &tls).await;
                reporter.report_ingress(&outcome).await;
                let _ = tx.send(outcome);
            });
        }
    });
    Ok(Harness {
        addr,
        outcomes,
        resolver,
        sink,
    })
}

async fn echo_upstream() -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    Ok(port)
}

fn basic(password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{password}")))
}

async fn read_head(stream: &mut TcpStream) -> TestResult<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).await?;
        assert!(n > 0, "closed early: {:?}", String::from_utf8_lossy(&buf));
        buf.push(byte[0]);
    }
    Ok(String::from_utf8(buf)?)
}

async fn http_connect(proxy: SocketAddr, target: &str, auth: Option<&str>) -> TestResult<(TcpStream, String)> {
    let mut stream = TcpStream::connect(proxy).await?;
    let auth = auth
        .map(|a| format!("Proxy-Authorization: {a}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth}\r\n").as_bytes())
        .await?;
    let head = read_head(&mut stream).await?;
    Ok((stream, head))
}

async fn socks_handshake(stream: &mut TcpStream, password: &str) -> TestResult<[u8; 2]> {
    socks_login(stream, "sandbox", password).await
}

async fn socks_login(stream: &mut TcpStream, user: &str, password: &str) -> TestResult<[u8; 2]> {
    stream.write_all(&[5, 1, 2]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(password.len() as u8);
    auth.extend_from_slice(password.as_bytes());
    stream.write_all(&auth).await?;
    let mut status = [0u8; 2];
    stream.read_exact(&mut status).await?;
    Ok(status)
}

async fn socks_request(stream: &mut TcpStream, command: u8, atyp: u8, address: &[u8], port: u16) -> TestResult<u8> {
    let mut request = vec![5, command, 0, atyp];
    request.extend_from_slice(address);
    request.extend(port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await?;
    Ok(reply[1])
}

fn domain_address(name: &str) -> Vec<u8> {
    let mut v = vec![name.len() as u8];
    v.extend_from_slice(name.as_bytes());
    v
}

async fn echo_roundtrip(stream: &mut TcpStream) -> TestResult {
    stream.write_all(b"ping").await?;
    let mut got = [0u8; 4];
    stream.read_exact(&mut got).await?;
    assert_eq!(&got, b"ping");
    Ok(())
}

#[tokio::test]
async fn http_connect_tunnels_to_an_allowed_domain() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut h = start(IngressLimits::default()).await?;
    let (mut stream, head) = http_connect(h.addr, &format!("allowed.test:{upstream}"), Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut stream).await?;
    drop(stream);
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Completed { .. },
            dest_ip,
            ..
        } => assert_eq!(dest_ip, Some(IpAddr::V4(Ipv4Addr::LOCALHOST))),
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

async fn closed_port() -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.port())
}

#[tokio::test]
async fn http_connect_answers_502_and_never_200_when_the_dial_fails() -> TestResult {
    let closed = closed_port().await?;
    let mut h = start(IngressLimits::default()).await?;
    let (mut stream, head) = http_connect(h.addr, &format!("allowed.test:{closed}"), Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{head}");
    assert!(!head.contains(" 200 "), "{head}");
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await?;
    assert!(rest.is_empty());
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::UpstreamConnectFailed(_),
            ..
        } => {}
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn http_connect_falls_back_to_the_next_vetted_address() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut network = policy()?;
    if let NetworkPolicy::UserDefined { allow_ranges, .. } = &mut network {
        allow_ranges.push(AllowRange::new("::1/128")?);
    }
    let mut h = start_resolving(
        IngressLimits::default(),
        vec![
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        network,
    )
    .await?;
    let (mut stream, head) = http_connect(h.addr, &format!("allowed.test:{upstream}"), Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut stream).await?;
    drop(stream);
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Completed { .. },
            ..
        } => {}
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

fn allowing_v6_loopback() -> TestResult<NetworkPolicy> {
    let mut network = policy()?;
    if let NetworkPolicy::UserDefined { allow_ranges, .. } = &mut network {
        allow_ranges.push(AllowRange::new("::1/128")?);
    }
    Ok(network)
}

#[tokio::test]
async fn http_connect_takes_the_vetted_addresses_in_turn_past_a_repeated_family() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut h = start_resolving(
        IngressLimits::default(),
        vec![
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        ],
        allowing_v6_loopback()?,
    )
    .await?;
    let (mut stream, head) = http_connect(h.addr, &format!("allowed.test:{upstream}"), Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut stream).await?;
    drop(stream);
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Completed { .. },
            ..
        } => {}
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn http_connect_answers_502_only_once_every_vetted_address_refuses() -> TestResult {
    let closed = closed_port().await?;
    let mut h = start_resolving(
        IngressLimits::default(),
        vec![
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        allowing_v6_loopback()?,
    )
    .await?;
    let (mut stream, head) = http_connect(h.addr, &format!("allowed.test:{closed}"), Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{head}");
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await?;
    assert!(rest.is_empty());
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::UpstreamConnectFailed(_),
            ..
        } => {}
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn socks5_falls_back_to_the_next_vetted_address() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut network = policy()?;
    if let NetworkPolicy::UserDefined { allow_ranges, .. } = &mut network {
        allow_ranges.push(AllowRange::new("::1/128")?);
    }
    let h = start_resolving(
        IngressLimits::default(),
        vec![
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ],
        network,
    )
    .await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 3, &domain_address("allowed.test"), upstream).await?;
    assert_eq!(reply, 0);
    echo_roundtrip(&mut stream).await?;
    Ok(())
}

#[tokio::test]
async fn http_connect_denial_has_a_stable_contract_and_emits_a_violation() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let (_stream, head) = http_connect(h.addr, "blocked.test:443", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    assert!(head.contains("X-Proxy-Error: blocked-by-allowlist\r\n"));
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Denied(_),
            dest_ip: None,
            ..
        } => {}
        other => return Err(format!("{other:?}").into()),
    }
    assert_eq!(
        h.resolver.calls.load(Ordering::SeqCst),
        0,
        "denied name must not be resolved"
    );
    let events = lock(&h.sink.0)?;
    assert_eq!(events.len(), 1);
    let event: EgressViolation = serde_json::from_slice(&events[0])?;
    assert_eq!(event.host.as_deref(), Some("blocked.test"));
    assert_eq!(event.port, 443);
    assert_eq!(event.dest_ip, None);
    Ok(())
}

#[tokio::test]
async fn ip_literal_in_a_deny_range_is_denied_over_http_connect() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let (_s, head) = http_connect(h.addr, "10.1.2.3:443", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    Ok(())
}

fn metadata() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))
}

fn assert_floor_violation(h: &Harness, dest: &str) -> TestResult {
    let events = lock(&h.sink.0)?;
    assert_eq!(events.len(), 1);
    let event: serde_json::Value = serde_json::from_slice(&events[0])?;
    assert_eq!(event["host"], "allowed.test");
    assert_eq!(event["dest_ip"], dest);
    assert!(
        event["reason"].to_string().contains("destination_address_denied"),
        "{event}"
    );
    Ok(())
}

#[tokio::test]
async fn http_connect_to_a_name_resolving_to_metadata_is_refused_and_reported() -> TestResult {
    let mut h = start_answering(IngressLimits::default(), metadata()).await?;
    let (_s, head) = http_connect(h.addr, "allowed.test:80", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert!(!head.contains("169.254"));
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Denied(_),
            ..
        }
    ));
    assert_floor_violation(&h, "169.254.169.254")?;
    Ok(())
}

#[tokio::test]
async fn http_connect_to_a_name_resolving_to_a_denied_address_names_the_class() -> TestResult {
    let h = start_answering(IngressLimits::default(), metadata()).await?;
    let (mut stream, head) = http_connect(h.addr, "allowed.test:80", Some(&basic(TOKEN))).await?;
    assert_eq!(
        head,
        "HTTP/1.1 403 resolved to a link-local address\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\n\r\n"
    );
    let mut body = String::new();
    stream.read_to_string(&mut body).await?;
    assert_eq!(
        body,
        "Connection to allowed.test blocked: resolved to a link-local address"
    );
    Ok(())
}

#[tokio::test]
async fn http_connect_to_a_name_resolving_to_a_private_address_is_refused_by_default() -> TestResult {
    let h = start_answering(IngressLimits::default(), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))).await?;
    let (_stream, head) = http_connect(h.addr, "allowed.test:80", Some(&basic(TOKEN))).await?;
    assert!(
        head.starts_with("HTTP/1.1 403 resolved to a listed address\r\n"),
        "{head}"
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_to_a_name_resolving_to_metadata_is_refused() -> TestResult {
    let h = start_answering(IngressLimits::default(), metadata()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn socks5_to_a_name_resolving_to_metadata_is_refused() -> TestResult {
    let h = start_answering(IngressLimits::default(), metadata()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 3, &domain_address("allowed.test"), 80).await?;
    assert_eq!(reply, 2);
    Ok(())
}

#[tokio::test]
async fn a_metadata_ip_literal_is_refused_even_though_no_rule_names_it() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let (_s, head) = http_connect(h.addr, "169.254.169.254:80", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn unauthenticated_http_gets_407_and_nothing_about_policy() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    for auth in [None, Some(basic("wrong"))] {
        let (_s, head) = http_connect(h.addr, "blocked.test:443", auth.as_deref()).await?;
        assert!(head.starts_with("HTTP/1.1 407 "), "{head}");
        assert!(head.contains("X-Proxy-Error: unauthenticated"));
        assert!(!head.contains("blocked"));
        assert!(matches!(
            h.outcomes.recv().await.ok_or("the outcome channel closed")?,
            IngressOutcome::Rejected(IngressRejectionError::Unauthenticated)
        ));
    }
    assert!(lock(&h.sink.0)?.is_empty());
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn socks5_connects_by_domain_and_by_ip_literal() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut h = start(IngressLimits::default()).await?;

    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 3, &domain_address("allowed.test"), upstream).await?;
    assert_eq!(reply, 0);
    echo_roundtrip(&mut stream).await?;
    drop(stream);
    h.outcomes.recv().await.ok_or("the outcome channel closed")?;

    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 1, &[127, 0, 0, 1], upstream).await?;
    assert_eq!(reply, 0);
    echo_roundtrip(&mut stream).await?;
    drop(stream);
    h.outcomes.recv().await.ok_or("the outcome channel closed")?;

    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(
        &mut stream,
        1,
        4,
        &Ipv4Addr::LOCALHOST.to_ipv6_mapped().octets(),
        upstream,
    )
    .await?;
    assert_eq!(reply, 0);
    echo_roundtrip(&mut stream).await?;
    Ok(())
}

#[tokio::test]
async fn socks5_ip_literal_is_judged_without_dns() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 1, &[10, 9, 9, 9], 443).await?;
    assert_eq!(reply, 0x02);
    match h.outcomes.recv().await.ok_or("the outcome channel closed")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Denied(_),
            dest_ip: Some(ip),
            ..
        } => assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9))),
        other => return Err(format!("{other:?}").into()),
    }
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn socks5_refuses_bind_and_udp_associate() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    for command in [2u8, 3] {
        let mut stream = TcpStream::connect(h.addr).await?;
        assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
        stream.write_all(&[5, command, 0, 1, 127, 0, 0, 1, 0, 80]).await?;
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await?;
        assert_eq!(reply, [5, 0x07]);
    }
    Ok(())
}

#[tokio::test]
async fn socks5_wrong_token_is_refused_and_anonymous_gets_the_unauthenticated_answer() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, "wrong").await?, [1, 1]);
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::Unauthenticated)
    ));

    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    assert_eq!(method, [5, 0]);
    Ok(())
}

#[tokio::test]
async fn a_socks5_login_needs_the_sandbox_user_name() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    for (user, status) in [("sandbox", 0), ("sandbox.cmd", 0), ("sandboxx", 1), ("u", 1), ("", 1)] {
        let mut stream = TcpStream::connect(h.addr).await?;
        assert_eq!(socks_login(&mut stream, user, TOKEN).await?, [1, status], "{user}");
    }
    Ok(())
}

#[tokio::test]
async fn plain_http_is_forwarded_in_origin_form_without_proxy_credentials() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let received = Arc::new(Mutex::new(String::new()));
    let seen = received.clone();
    tokio::spawn(async move {
        let (mut probe, _) = listener.accept().await?;
        let mut probed = Vec::new();
        probe.read_to_end(&mut probed).await?;
        assert!(probed.is_empty(), "{probed:?}");
        let (mut stream, _) = listener.accept().await?;
        let head = read_head(&mut stream).await?;
        *lock(&seen)? = head;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await?;
        TestResult::Ok(())
    });

    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://allowed.test:{port}/b?x=1 HTTP/1.1\r\nHost: allowed.test:{port}\r\nProxy-Authorization: {}\r\nProxy-Connection: keep-alive\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("ok"));

    let upstream_saw = lock(&received)?.clone();
    assert_eq!(
        upstream_saw,
        format!("GET /b?x=1 HTTP/1.1\r\nhost: allowed.test:{port}\r\nConnection: close\r\n\r\n")
    );
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Completed { .. },
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn plain_http_to_a_denied_name_gets_403() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://blocked.test/ HTTP/1.1\r\nHost: blocked.test\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 403 "));
    assert!(head.contains("X-Proxy-Error: blocked-by-allowlist"));
    Ok(())
}

async fn plain_exchange(request_tail: &str, host: &str) -> TestResult<String> {
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://{host}/ HTTP/1.1\r\n{request_tail}Proxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response).await;
    Ok(response)
}

#[tokio::test]
async fn a_denied_name_is_refused_as_denied_before_its_host_header_is_compared() -> TestResult {
    let response = plain_exchange("Host: elsewhere.test\r\n", "blocked.test").await?;
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    assert!(response.contains("X-Proxy-Error: blocked-by-allowlist"));
    Ok(())
}

#[tokio::test]
async fn an_allowed_name_with_another_host_header_gets_421_in_the_words_of_the_proxy() -> TestResult {
    let response = plain_exchange("Host: elsewhere.test\r\n", "allowed.test").await?;
    assert!(response.starts_with("HTTP/1.1 421 "), "{response}");
    assert!(
        response.ends_with(
            "Transfer-Encoding: chunked\r\n\r\n2e\r\nHost header does not match the request target\n\r\n0\r\n\r\n"
        ),
        "{response}"
    );
    Ok(())
}

#[tokio::test]
async fn host_headers_that_do_not_spell_the_target_exactly_get_421() -> TestResult {
    for host in ["allowed.test.", "ALLOWED.test:81", "allowed.test:", "allowed.test:080"] {
        let response = plain_exchange(&format!("Host: {host}\r\n"), "allowed.test").await?;
        assert!(response.starts_with("HTTP/1.1 421 "), "{host}: {response}");
    }
    let response = plain_exchange("Host: allowed.test\r\nHost: allowed.test\r\n", "allowed.test").await?;
    assert!(response.starts_with("HTTP/1.1 421 "), "{response}");
    assert!(
        response.ends_with("Transfer-Encoding: chunked\r\n\r\n1a\r\nmore than one Host header\n\r\n0\r\n\r\n"),
        "{response}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_first_byte_gets_a_400_as_the_proxy_answers_it() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(&[0x16, 0x03, 0x01]).await?;
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest).await;
    assert!(rest.starts_with(b"HTTP/1.1 400 Bad Request\r\n"), "{rest:?}");
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::UnknownProtocol)
    ));
    Ok(())
}

#[tokio::test]
async fn a_slow_request_head_is_bounded_by_the_head_timeout_not_the_handshake_timeout() -> TestResult {
    let limits = IngressLimits {
        handshake_timeout: Duration::from_millis(100),
        head_timeout: Duration::from_secs(5),
        ..IngressLimits::default()
    };
    let h = start(limits).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(b"GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\n")
        .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    stream.write_all(b"\r\n").await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 407 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn a_connect_target_that_is_not_a_plain_host_is_denied_as_malformed_not_a_bad_request() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    for target in ["u@a.test:443", "a/b:443"] {
        let (_stream, head) = http_connect(h.addr, target, Some(&basic(TOKEN))).await?;
        assert!(head.starts_with("HTTP/1.1 403 "), "{target}: {head}");
        assert!(matches!(
            h.outcomes.recv().await.ok_or("the outcome channel closed")?,
            IngressOutcome::Rejected(IngressRejectionError::MalformedHost { port: 443, .. })
        ));
    }
    for target in ["a.test", "a:b:443", "a.test:0", "a.test:65536"] {
        let (_stream, head) = http_connect(h.addr, target, Some(&basic(TOKEN))).await?;
        assert!(head.starts_with("HTTP/1.1 400 "), "{target}: {head}");
    }
    Ok(())
}

#[tokio::test]
async fn silent_clients_time_out() -> TestResult {
    let limits = IngressLimits {
        handshake_timeout: Duration::from_millis(150),
        head_timeout: Duration::from_millis(150),
        ..IngressLimits::default()
    };
    let mut h = start(limits).await?;

    let _silent = TcpStream::connect(h.addr).await?;
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::HandshakeTimedOut)
    ));

    let mut slow = TcpStream::connect(h.addr).await?;
    slow.write_all(b"CONNECT a.test:443 HTTP/1.1\r\n").await?;
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::HandshakeTimedOut)
    ));
    Ok(())
}

#[tokio::test]
async fn the_tunnel_cap_does_not_count_tunnels_that_are_not_terminated() -> TestResult {
    let upstream = echo_upstream().await?;
    let h = start_with_options(NetworkOptions::default().with_tunnel_cap(TunnelCap::new(1)?)).await?;
    let target = format!("allowed.test:{upstream}");
    let mut tunnels = Vec::new();
    for _ in 0..3 {
        let (mut tunnel, head) = http_connect(h.addr, &target, Some(&basic(TOKEN))).await?;
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        echo_roundtrip(&mut tunnel).await?;
        tunnels.push(tunnel);
    }
    Ok(())
}

#[tokio::test]
async fn client_abort_mid_tunnel_does_not_wedge_the_listener() -> TestResult {
    let upstream = echo_upstream().await?;
    let h = start(IngressLimits::default()).await?;
    let target = format!("allowed.test:{upstream}");
    for _ in 0..3 {
        let (stream, head) = http_connect(h.addr, &target, Some(&basic(TOKEN))).await?;
        assert!(head.starts_with("HTTP/1.1 200 "));
        drop(stream);
    }
    let (mut stream, _) = http_connect(h.addr, &target, Some(&basic(TOKEN))).await?;
    echo_roundtrip(&mut stream).await?;
    Ok(())
}

type Seen = Arc<Mutex<Vec<Vec<u8>>>>;

const OK_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

fn complete_request_len(buf: &[u8]) -> Option<usize> {
    let head_end = find(buf, b"\r\n\r\n", 0)? + 4;
    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
    if head.contains("transfer-encoding: chunked") {
        let zero = if buf[head_end..].starts_with(b"0\r\n") {
            head_end
        } else {
            find(buf, b"\r\n0\r\n", head_end)? + 2
        };
        return Some(find(buf, b"\r\n\r\n", zero + 1)? + 4);
    }
    let length = match head.lines().find_map(|line| line.strip_prefix("content-length:")) {
        Some(value) => value.trim().parse::<usize>().ok()?,
        None => 0,
    };
    (buf.len() >= head_end + length).then_some(head_end + length)
}

async fn scripted_upstream(response: &'static [u8]) -> TestResult<(u16, Seen)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let record = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let record = record.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    while let Some(len) = complete_request_len(&buf) {
                        if let Ok(mut guard) = record.lock() {
                            guard.push(buf.drain(..len).collect());
                        }
                        if stream.write_all(response).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    Ok((port, seen))
}

async fn read_response(stream: &mut TcpStream) -> TestResult<(String, String)> {
    let head = read_head(stream).await?;
    let field = |wanted: &str| {
        head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(wanted).then(|| value.trim().to_owned())
        })
    };
    let mut body = Vec::new();
    if field("transfer-encoding").is_some_and(|coding| coding.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size_line = Vec::new();
            while !size_line.ends_with(b"\r\n") {
                size_line.push(stream.read_u8().await?);
            }
            let size = usize::from_str_radix(std::str::from_utf8(&size_line)?.trim_end(), 16)?;
            let mut chunk = vec![0u8; size + 2];
            stream.read_exact(&mut chunk).await?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else {
        let length = match field("content-length") {
            Some(value) => value.parse::<usize>()?,
            None => 0,
        };
        body.resize(length, 0);
        stream.read_exact(&mut body).await?;
    }
    Ok((head, String::from_utf8(body)?))
}

fn proxied(method: &str, port: u16, path: &str, extra: &str, body: &str) -> String {
    format!(
        "{method} http://allowed.test:{port}{path} HTTP/1.1\r\nHost: allowed.test:{port}\r\nProxy-Authorization: {}\r\n{extra}\r\n{body}",
        basic(TOKEN)
    )
}

fn rules_policy(request_rules: Vec<RequestRule>) -> TestResult<NetworkPolicy> {
    Ok(match policy()? {
        NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            ..
        } => NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            request_rules,
        },
        other => other,
    })
}

fn deny_rule(
    methods: &[&str],
    path_prefix: Option<&str>,
    header: Option<(&str, Option<&str>)>,
) -> TestResult<RequestRule> {
    Ok(RequestRule {
        action: RequestRuleAction::Deny,
        domains: Vec::new(),
        methods: methods
            .iter()
            .map(|method| HttpMethod::new(*method))
            .collect::<Result<_, _>>()?,
        path_prefix: path_prefix.map(PathPrefix::new).transpose()?,
        headers: header
            .map(|(name, value)| {
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(HeaderCondition {
                    name: HeaderName::new(name)?,
                    value: value.map(HeaderValue::new).transpose()?,
                })
            })
            .transpose()?
            .into_iter()
            .collect(),
    })
}

async fn read_until(stream: &mut TcpStream, end: &[u8]) -> TestResult<String> {
    let mut buf = Vec::new();
    while !buf.ends_with(end) {
        buf.push(stream.read_u8().await?);
    }
    Ok(undated(&String::from_utf8(buf)?))
}

fn undated(answer: &str) -> String {
    answer
        .split_inclusive("\r\n")
        .map(|line| {
            if line.starts_with("Date: ") {
                "Date: <DATE>\r\n"
            } else {
                line
            }
        })
        .collect()
}

async fn drain(stream: &mut TcpStream) -> TestResult<String> {
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest).await;
    Ok(String::from_utf8_lossy(&rest).into_owned())
}

#[tokio::test]
async fn plain_http_relays_a_chunked_request_body_in_its_framing() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let body = "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    let request = proxied("POST", port, "/up", "Transfer-Encoding: chunked\r\n", body);
    stream.write_all(request.as_bytes()).await?;
    let (head, answer) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(answer, "ok");
    let seen = lock(&seen)?;
    assert_eq!(seen.len(), 1);
    let sent = String::from_utf8(seen[0].clone())?;
    assert!(sent.starts_with("POST /up HTTP/1.1\r\nhost: allowed.test:"), "{sent}");
    assert!(sent.contains("transfer-encoding: chunked\r\n"));
    assert!(sent.ends_with(body), "{sent}");
    Ok(())
}

#[tokio::test]
async fn plain_http_serves_several_requests_per_connection_and_checks_each() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let network = rules_policy(vec![deny_rule(&[], Some("/admin"), None)?])?;
    let mut h = start_with(IngressLimits::default(), IpAddr::V4(Ipv4Addr::LOCALHOST), network).await?;
    let mut stream = TcpStream::connect(h.addr).await?;

    stream
        .write_all(proxied("GET", port, "/public", "", "").as_bytes())
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");

    stream
        .write_all(proxied("GET", port, "/public/two", "", "").as_bytes())
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");

    stream
        .write_all(proxied("GET", port, "/admin/users", "", "").as_bytes())
        .await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert!(head.contains("X-Proxy-Error: blocked-by-egress-proxy"), "{head}");

    assert_eq!(lock(&seen)?.len(), 2);
    drain(&mut stream).await?;
    let _ = h.outcomes.recv().await.ok_or("the outcome channel closed")?;
    let events = lock(&h.sink.0)?;
    assert_eq!(events.len(), 1);
    let event: EgressViolation = serde_json::from_slice(&events[0])?;
    assert_eq!(event.reason, EgressViolationReason::RequestDenied);
    assert_eq!(event.host.as_deref(), Some("allowed.test"));
    let request = event.request.ok_or("the refused request")?;
    assert_eq!(request.method(), "GET");
    assert_eq!(request.target(), format!("http://allowed.test:{port}/admin/users"));
    assert!(!request.reason().is_empty());
    Ok(())
}

#[tokio::test]
async fn plain_http_request_rules_match_method_and_header() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let network = rules_policy(vec![
        deny_rule(&["DELETE"], None, None)?,
        deny_rule(&[], None, Some(("x-blocked", Some("yes"))))?,
    ])?;
    let h = start_with(IngressLimits::default(), IpAddr::V4(Ipv4Addr::LOCALHOST), network).await?;

    for (request, allowed) in [
        (proxied("DELETE", port, "/x", "", ""), false),
        (proxied("GET", port, "/x", "X-Blocked: yes\r\n", ""), false),
        (proxied("GET", port, "/x", "X-Blocked: no\r\n", ""), true),
        (proxied("GET", port, "/x", "", ""), true),
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        let head = read_head(&mut stream).await?;
        let expected = if allowed { "HTTP/1.1 200 " } else { "HTTP/1.1 403 " };
        assert!(head.starts_with(expected), "{request:?} -> {head}");
    }
    assert_eq!(lock(&seen)?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn plain_http_denial_gives_the_client_the_whole_answer_despite_a_late_body() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let network = rules_policy(vec![deny_rule(&["POST"], None, None)?])?;
    let h = start_with(IngressLimits::default(), IpAddr::V4(Ipv4Addr::LOCALHOST), network).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let body = "a".repeat(300_000);
    let head = proxied("POST", port, "/up", &format!("Content-Length: {}\r\n", body.len()), "");
    let (mut read, mut write) = stream.split();
    let writer = async {
        let first = write.write_all(head.as_bytes()).await;
        let _ = write.write_all(body.as_bytes()).await;
        first
    };
    let reader = async {
        let mut all = Vec::new();
        read.read_to_end(&mut all).await?;
        io::Result::Ok(String::from_utf8_lossy(&all).into_owned())
    };
    let (written, answer) = tokio::join!(writer, reader);
    written?;
    let answer = answer?;
    assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    assert!(answer.contains("denied by sandbox policy\n"), "{answer}");
    assert!(lock(&seen)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn plain_http_refuses_trace_before_any_lookup() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    for (request, status) in [(proxied("TRACE", port, "/", "", ""), "405")] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        let answer = drain(&mut stream).await?;
        assert!(
            answer.starts_with(&format!("HTTP/1.1 {status} ")),
            "{request:?} -> {answer}"
        );
    }
    assert!(lock(&seen)?.is_empty());
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn plain_http_refuses_request_smuggling_shapes() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    for request in [
        proxied(
            "POST",
            port,
            "/",
            "Content-Length: 4\r\nTransfer-Encoding: chunked\r\n",
            "0\r\n\r\n",
        ),
        proxied("POST", port, "/", "Content-Length: 4\r\nContent-Length: 5\r\n", "abcd"),
        proxied("POST", port, "/", "Content-Length: 4\r\nContent-Length: 4\r\n", "abcd"),
        proxied("GET", port, "/", "X-Folded: a\r\n b\r\n", ""),
        proxied("POST", port, "/", "Transfer-Encoding: chunked, gzip\r\n", "0\r\n\r\n"),
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        let answer = drain(&mut stream).await?;
        assert!(answer.starts_with("HTTP/1.1 4"), "{request:?} -> {answer}");
    }
    assert!(lock(&seen)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn plain_http_refuses_an_invalid_chunk_size_without_completing_the_upstream_request() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(
            proxied(
                "POST",
                port,
                "/",
                "Transfer-Encoding: chunked\r\n",
                "zz\r\nabc\r\n0\r\n\r\n",
            )
            .as_bytes(),
        )
        .await?;
    let answer = drain(&mut stream).await?;
    assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
    assert!(lock(&seen)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn plain_http_keeps_a_request_after_chunked_trailers_apart() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let network = rules_policy(vec![deny_rule(&[], Some("/admin"), None)?])?;
    let h = start_with(IngressLimits::default(), IpAddr::V4(Ipv4Addr::LOCALHOST), network).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let first = proxied(
        "POST",
        port,
        "/up",
        "Transfer-Encoding: chunked\r\n",
        "3\r\nabc\r\n0\r\nX-Trailer: GET /admin HTTP/1.1\r\n\r\n",
    );
    let second = proxied("GET", port, "/admin", "", "");
    stream.write_all(format!("{first}{second}").as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(lock(&seen)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn plain_http_client_abort_mid_body_never_completes_the_upstream_request() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(proxied("POST", port, "/", "Content-Length: 100\r\n", "abcd").as_bytes())
        .await?;
    drop(stream);
    let _ = h.outcomes.recv().await.ok_or("the outcome channel closed")?;
    assert!(lock(&seen)?.is_empty());

    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    Ok(())
}

#[tokio::test]
async fn plain_http_answers_502_to_an_invalid_upstream_response_and_keeps_serving() -> TestResult {
    let (bad_port, _) = scripted_upstream(b"HTTP/1.1 99 Nope\r\n\r\n").await?;
    let (good_port, _) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;

    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(proxied("GET", bad_port, "/", "", "").as_bytes())
        .await?;
    let answer = drain(&mut stream).await?;
    assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    assert!(!answer.contains("X-Proxy-Error"), "{answer}");

    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(proxied("GET", good_port, "/", "", "").as_bytes())
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    Ok(())
}

#[tokio::test]
async fn plain_http_forwards_the_target_without_proxy_headers() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(
            proxied(
                "GET",
                port,
                "/p%20q?x=1&y=2",
                "Proxy-Connection: keep-alive\r\nConnection: keep-alive, X-Hop\r\nX-Hop: 1\r\nAccept: */*\r\n",
                "",
            )
            .as_bytes(),
        )
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let sent = String::from_utf8(lock(&seen)?[0].clone())?;
    assert_eq!(
        sent,
        format!("GET /p%20q?x=1&y=2 HTTP/1.1\r\nhost: allowed.test:{port}\r\naccept: */*\r\nConnection: close\r\n\r\n")
    );
    Ok(())
}

#[tokio::test]
async fn http_connect_denial_has_the_documented_response() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let (mut stream, head) = http_connect(h.addr, "blocked.test:443", Some(&basic(TOKEN))).await?;
    assert_eq!(
        head,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-allowlist\r\n\r\n"
    );
    let mut body = String::new();
    stream.read_to_string(&mut body).await?;
    assert_eq!(body, "Connection blocked by network allowlist");
    Ok(())
}

#[tokio::test]
async fn reasoned_refusals_word_a_connect_denial_with_its_reason() -> TestResult {
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    let (_s, head) = http_connect(h.addr, "blocked.test:443", Some(&basic(TOKEN))).await?;
    assert_eq!(
        head,
        "HTTP/1.1 403 host is on the deny list\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-allowlist\r\n\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn reasoned_refusals_challenge_without_a_mark() -> TestResult {
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    let (_s, head) = http_connect(h.addr, "allowed.test:443", None).await?;
    assert_eq!(
        head,
        "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"sandbox\"\r\n\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_denial_body_ends_in_a_newline() -> TestResult {
    let (port, _seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://blocked.test/ HTTP/1.1\r\nHost: blocked.test\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    assert_eq!(
        read_until(&mut stream, b"0\r\n\r\n").await?,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-allowlist\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n28\r\nConnection blocked by network allowlist\n\r\n0\r\n\r\n"
    );
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let (head, body) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    assert_eq!(body, "ok");
    Ok(())
}

#[tokio::test]
async fn socks4_requests_are_closed_without_a_reply_as_the_socks_library_does() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(&[0x04, 0x01, 0x01, 0xbb, 127, 0, 0, 1, 0]).await?;
    let mut reply = Vec::new();
    let read = stream.read_to_end(&mut reply).await;
    assert!(matches!(read, Ok(0)), "a reset instead of a clean close: {read:?}");
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::UnsupportedSocksVersion)
    ));
    Ok(())
}

fn connect_with_padding(padding: usize) -> String {
    format!(
        "CONNECT allowed.test:443 HTTP/1.1\r\nHost: allowed.test:443\r\nProxy-Authorization: {}\r\nX-Pad: {}\r\n\r\n",
        basic(TOKEN),
        "a".repeat(padding)
    )
}

#[tokio::test]
async fn the_default_head_limit_is_sixteen_kib() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(connect_with_padding(20 * 1024).as_bytes()).await?;
    let mut reply = Vec::new();
    let _ = stream.read_to_end(&mut reply).await;
    assert!(
        String::from_utf8_lossy(&reply).starts_with("HTTP/1.1 431 "),
        "{}",
        String::from_utf8_lossy(&reply)
    );
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::HeadTooLarge)
    ));
    Ok(())
}

#[tokio::test]
async fn the_proxy_only_head_limit_accepts_what_the_default_refuses() -> TestResult {
    assert_eq!(IngressLimits::default().max_head_bytes, 16 * 1024);
    assert_eq!(IngressLimits::proxy_only().max_head_bytes, 2 * 1024 * 1024);
    let h = start(IngressLimits::proxy_only()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(connect_with_padding(20 * 1024).as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(!head.starts_with("HTTP/1.1 431 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn a_proxy_login_without_a_user_name_is_refused_as_documented() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    for credentials in [format!(":{TOKEN}"), TOKEN.to_string()] {
        let auth = format!("Basic {}", STANDARD.encode(credentials));
        let (_stream, head) = http_connect(h.addr, "allowed.test:443", Some(&auth)).await?;
        assert!(
            head.starts_with("HTTP/1.1 407 Proxy Authentication Required\r\n"),
            "{head}"
        );
        assert!(
            head.contains("Proxy-Authenticate: Basic realm=\"sandbox\"\r\n"),
            "{head}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn plain_http_to_a_name_resolving_to_a_denied_address_names_the_class() -> TestResult {
    let h = start_answering(IngressLimits::default(), metadata()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    let refused = "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n45\r\nConnection to allowed.test blocked: resolved to a link-local address\n\r\n0\r\n\r\n";
    for _ in 0..2 {
        stream.write_all(request.as_bytes()).await?;
        assert_eq!(read_until(&mut stream, b"0\r\n\r\n").await?, refused);
    }
    Ok(())
}

#[tokio::test]
async fn every_class_among_the_denied_addresses_is_named_in_the_documented_order() -> TestResult {
    let h = start_resolving(
        IngressLimits::default(),
        vec!["fd00:ec2::254".parse()?, IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))],
        policy()?,
    )
    .await?;
    let (_stream, head) = http_connect(h.addr, "allowed.test:80", Some(&basic(TOKEN))).await?;
    assert!(
        head.starts_with("HTTP/1.1 403 resolved to a multicast address / a cloud metadata address\r\n"),
        "{head}"
    );
    Ok(())
}

#[tokio::test]
async fn a_bracketed_ipv6_denied_domain_blocks_a_connect_to_that_literal_in_any_spelling() -> TestResult {
    let network = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("::/0")?],
        deny_ranges: vec![],
        denied_domains: vec!["[2001:db8::1]:443".parse()?, "[fd00:ec2::254]".parse()?],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    };
    let h = start_with(IngressLimits::default(), "2001:db8::1".parse()?, network).await?;
    for target in ["[2001:DB8:0::1]:443", "[2001:db8::1]:443", "[fd00:ec2::254]:80"] {
        let (_stream, head) = http_connect(h.addr, target, Some(&basic(TOKEN))).await?;
        assert!(head.starts_with("HTTP/1.1 403 "), "{target}: {head}");
    }
    Ok(())
}

#[tokio::test]
async fn socks5_without_the_login_method_gets_the_unauthenticated_answer_and_a_port_22_refusal() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    assert_eq!(method, [5, 0]);
    let reply = socks_request(&mut stream, 1, 3, &domain_address("allowed.test"), 22).await?;
    assert_eq!(reply, 0);
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await?;
    assert!(rest.starts_with(b"SSH-2.0-policy_refusal\r\n"));
    assert!(
        String::from_utf8_lossy(&rest)
            .contains("This proxy requires authentication, and this client did not offer an authentication method, so the connection was refused.")
    );
    Ok(())
}

#[tokio::test]
async fn socks5_without_the_login_method_is_refused_on_other_ports_and_a_denied_host_is_reported() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    let reply = socks_request(&mut stream, 1, 3, &domain_address("blocked.test"), 80).await?;
    assert_eq!(reply, 2);
    h.outcomes.recv().await.ok_or("the outcome channel closed")?;
    let events = lock(&h.sink.0)?;
    assert_eq!(events.len(), 1);
    let event: serde_json::Value = serde_json::from_slice(&events[0])?;
    assert_eq!(event["host"], "blocked.test");
    Ok(())
}

#[tokio::test]
async fn socks5_with_a_malformed_host_is_refused_and_reported() -> TestResult {
    let mut h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 3, &domain_address("bad host!"), 80).await?;
    assert_eq!(reply, 2);
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::MalformedHost { .. })
    ));
    let events = lock(&h.sink.0)?;
    assert_eq!(events.len(), 1);
    let event: serde_json::Value = serde_json::from_slice(&events[0])?;
    assert_eq!(event["host"], "bad host!");
    assert!(event["reason"].to_string().contains("malformed_host"), "{event}");
    Ok(())
}

async fn crl_exchange(serves_crl: bool, request: &str) -> TestResult<(Vec<u8>, IngressOutcome, Vec<u8>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let tls = tls_context()?;
    let der = tls.ca.crl_der().to_vec();
    let lookup = Arc::new(StubLookup {
        calls: AtomicUsize::new(0),
        answers: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        delay: Duration::ZERO,
        held: None,
    });
    let mut ingress = Ingress::new(
        ProxyToken::new(TOKEN)?,
        Arc::new(VettingResolver::new(lookup, SsrfFloor::system())),
        IngressLimits::proxy_only(),
    );
    if serves_crl {
        ingress = ingress.with_crl(Arc::from(der.as_slice()));
    }
    let network = policy()?;
    let server = tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return None;
        };
        Some(ingress.serve(stream, &network, &tls).await)
    });
    let mut client = TcpStream::connect(addr).await?;
    client.write_all(request.as_bytes()).await?;
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await?;
    Ok((reply, server.await?.ok_or("no connection arrived")?, der))
}

fn split_reply(reply: &[u8]) -> TestResult<(String, &[u8])> {
    let end = reply
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the reply has no head end")?
        + 4;
    Ok((String::from_utf8(reply[..end].to_vec())?, &reply[end..]))
}

#[tokio::test]
async fn the_revocation_list_is_served_without_credentials() -> TestResult {
    for target in ["/ca.crl", "http://127.0.0.1:1/ca.crl", "/ca.crl?x=1"] {
        let (reply, outcome, der) = crl_exchange(
            true,
            &format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
        )
        .await?;
        let (head, body) = split_reply(&reply)?;
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        assert!(head.contains("Content-Type: application/pkix-crl\r\n"), "{head}");
        assert!(head.contains(&format!("Content-Length: {}\r\n", der.len())), "{head}");
        assert_eq!(body, der.as_slice());
        assert!(matches!(outcome, IngressOutcome::CrlServed), "{outcome:?}");
    }
    Ok(())
}

#[tokio::test]
async fn the_connection_stays_alive_after_the_revocation_list() -> TestResult {
    let (reply, outcome, der) = crl_exchange(
        true,
        "GET /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\nGET /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .await?;
    let (head, rest) = split_reply(&reply)?;
    assert!(head.contains("Connection: keep-alive\r\n"), "{head}");
    assert_eq!(&rest[..der.len()], der.as_slice());
    let (second, body) = split_reply(&rest[der.len()..])?;
    assert!(second.starts_with("HTTP/1.1 200 "), "{second}");
    assert!(second.contains("Connection: close\r\n"), "{second}");
    assert_eq!(body, der.as_slice());
    assert!(matches!(outcome, IngressOutcome::CrlServed), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_token_authenticated_request_after_the_revocation_list_is_served_as_usual() -> TestResult {
    let (port, _seen) = scripted_upstream(OK_RESPONSE).await?;
    let request = format!(
        "GET /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n{}",
        proxied("GET", port, "/", "Connection: close\r\n", "")
    );
    let (reply, outcome, der) = crl_exchange(true, &request).await?;
    let (_, rest) = split_reply(&reply)?;
    let (second, body) = split_reply(&rest[der.len()..])?;
    assert!(second.starts_with("HTTP/1.1 200 "), "{second}");
    assert!(!second.contains("pkix-crl"), "{second}");
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    assert_eq!(body, b"ok");
    Ok(())
}

#[tokio::test]
async fn the_revocation_list_needs_a_proxy_that_serves_it_and_a_get() -> TestResult {
    let (reply, _, _) = crl_exchange(false, "GET /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await?;
    assert!(split_reply(&reply)?.0.starts_with("HTTP/1.1 407 "));

    for request in [
        "POST /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
        "GET /other.crl HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
    ] {
        let (reply, outcome, _) = crl_exchange(true, request).await?;
        assert!(split_reply(&reply)?.0.starts_with("HTTP/1.1 407 "), "{request}");
        assert!(!matches!(outcome, IngressOutcome::CrlServed));
    }
    Ok(())
}

#[tokio::test]
async fn plain_http_relays_response_header_bytes_past_ascii_unchanged() -> TestResult {
    let (port, _seen) =
        scripted_upstream(b"HTTP/1.1 200 OK\r\nX-Latin: caf\xe9\r\nX-Utf8: caf\xc3\xa9\r\nContent-Length: 2\r\n\r\nok")
            .await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let mut raw = Vec::new();
    while !raw.ends_with(b"ok") {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await?;
        raw.push(byte[0]);
    }
    let has = |needle: &[u8]| raw.windows(needle.len()).any(|w| w == needle);
    assert!(raw.starts_with(b"HTTP/1.1 200 OK"));
    assert!(has(b"x-latin: caf\xe9\r\n"));
    assert!(has(b"x-utf8: caf\xc3\xa9\r\n"));
    Ok(())
}

#[tokio::test]
async fn plain_http_answers_a_non_utf8_target_with_400_and_forwards_nothing() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let mut request = proxied("GET", port, "/caf", "", "").into_bytes();
    let at = request
        .windows(4)
        .position(|w| w == b"/caf")
        .ok_or("the target is missing")?
        + 4;
    request.insert(at, 0xe9);
    stream.write_all(&request).await?;
    let reply = drain(&mut stream).await?;
    assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
    assert!(lock(&seen)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn plain_http_drops_hop_by_hop_fields_in_every_separator_spelling_and_request_trailers() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let extra = "Connection: keep-alive, X_Conn.Named\r\nX-Conn-Named: named\r\nKeep-Alive: timeout=5\r\nProxy-Connection: keep-alive\r\nTE: trailers\r\nTrailer: X-Trail\r\nUpgrade: h2c\r\nProxy-Authenticate: Basic\r\nProxy_Authorization: Basic Z3Vlc3Q6Z3Vlc3Q=\r\nKeep.Alive: timeout=5\r\nX-Kept: yes\r\nTransfer-Encoding: chunked\r\n";
    stream
        .write_all(
            proxied(
                "POST",
                port,
                "/up",
                extra,
                "5\r\nhello\r\n0\r\nX-Trail: trailer-value\r\n\r\n",
            )
            .as_bytes(),
        )
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let sent = String::from_utf8(lock(&seen)?[0].clone())?;
    assert_eq!(
        sent,
        format!(
            "POST /up HTTP/1.1\r\nhost: allowed.test:{port}\r\nx-kept: yes\r\ntransfer-encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n"
        )
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_frames_a_chunked_body_again_after_the_other_fields_as_the_reference_server_does() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(
            proxied(
                "POST",
                port,
                "/up",
                "Transfer-Encoding: chunked\r\nX-Kept: yes\r\n",
                "5\r\nhello\r\n0\r\n\r\n",
            )
            .as_bytes(),
        )
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let sent = String::from_utf8(lock(&seen)?[0].clone())?;
    assert_eq!(
        sent,
        format!(
            "POST /up HTTP/1.1\r\nhost: allowed.test:{port}\r\nx-kept: yes\r\ntransfer-encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n"
        )
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_keeps_content_length_while_dropping_hop_by_hop_fields() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(
            proxied(
                "POST",
                port,
                "/up",
                "Proxy-Connection: keep-alive\r\nConnection: close\r\nContent-Length: 5\r\n",
                "hello",
            )
            .as_bytes(),
        )
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let sent = String::from_utf8(lock(&seen)?[0].clone())?;
    assert_eq!(
        sent,
        format!(
            "POST /up HTTP/1.1\r\nhost: allowed.test:{port}\r\ncontent-length: 5\r\nConnection: close\r\n\r\nhello"
        )
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_forwards_expect_continue_and_the_client_sees_one_interim_answer() -> TestResult {
    let (port, seen) =
        scripted_upstream(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(
            proxied(
                "POST",
                port,
                "/up",
                "Expect: 100-continue\r\nConnection: close\r\nContent-Length: 5\r\n",
                "",
            )
            .as_bytes(),
        )
        .await?;
    let interim = read_head(&mut stream).await?;
    assert!(interim.starts_with("HTTP/1.1 100 Continue"), "{interim}");
    stream.write_all(b"hello").await?;
    let (head, body) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(body, "ok");
    let sent = String::from_utf8(lock(&seen)?[0].clone())?;
    assert_eq!(
        sent,
        format!(
            "POST /up HTTP/1.1\r\nhost: allowed.test:{port}\r\nexpect: 100-continue\r\ncontent-length: 5\r\nConnection: close\r\n\r\nhello"
        )
    );
    Ok(())
}

async fn slow_connect(h: &Harness, upstream: u16) -> TestResult<TcpStream> {
    let mut stream = TcpStream::connect(h.addr).await?;
    let target = format!("allowed.test:{upstream}");
    let auth = basic(TOKEN);
    stream
        .write_all(
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {auth}\r\n\r\n").as_bytes(),
        )
        .await?;
    Ok(stream)
}

async fn start_slow(capture: bool) -> TestResult<Harness> {
    start_deciding(
        IngressLimits::default(),
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        policy()?,
        Duration::from_millis(400),
        capture,
    )
    .await
}

#[tokio::test]
async fn bytes_sent_while_the_destination_is_decided_reach_the_tunnel() -> TestResult {
    let upstream = echo_upstream().await?;
    let h = start_slow(true).await?;
    let mut stream = slow_connect(&h, upstream).await?;
    stream.write_all(b"ping").await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    let mut got = [0u8; 4];
    stream.read_exact(&mut got).await?;
    assert_eq!(&got, b"ping");
    Ok(())
}

#[tokio::test]
async fn a_client_that_closes_during_the_decision_gets_no_answer() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut h = start_slow(true).await?;
    let mut stream = slow_connect(&h, upstream).await?;
    stream.shutdown().await?;
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await?;
    assert!(rest.is_empty(), "{:?}", String::from_utf8_lossy(&rest));
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::Io(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_client_that_sends_more_than_the_cap_during_the_decision_gets_no_answer() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut h = start_slow(true).await?;
    let mut stream = slow_connect(&h, upstream).await?;
    let _ = stream.write_all(&vec![b'x'; 256 * 1024]).await;
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest).await;
    assert!(!String::from_utf8_lossy(&rest).starts_with("HTTP/1.1 200"));
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::Io(_))
    ));
    Ok(())
}

#[tokio::test]
async fn without_the_capture_a_closing_client_still_gets_its_tunnel() -> TestResult {
    let upstream = echo_upstream().await?;
    let h = start_slow(false).await?;
    let mut stream = slow_connect(&h, upstream).await?;
    stream.write_all(b"ping").await?;
    stream.shutdown().await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    Ok(())
}

/// A proxy whose destination decision waits until the returned semaphore is
/// given a permit, and then answers `answer`, as the liveness tests of other clients hold
/// the filter's verdict (`http-proxy-verdict-liveness.test.ts:118-145`).
async fn start_with_held_verdict(capture: bool, answer: IpAddr) -> TestResult<(Harness, Arc<Semaphore>)> {
    let verdict = Arc::new(Semaphore::new(0));
    let h = start_holding(
        IngressLimits::default(),
        vec![answer],
        policy()?,
        Duration::ZERO,
        capture,
        Some(verdict.clone()),
    )
    .await?;
    Ok((h, verdict))
}

const DENIED_ANSWER: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

async fn decision_begun(h: &Harness) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.resolver.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| "the decision never began")?;
    Ok(())
}

async fn last_word_upstream() -> TestResult<(u16, mpsc::UnboundedReceiver<Vec<u8>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, got) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut seen = Vec::new();
        stream.read_to_end(&mut seen).await?;
        stream
            .write_all(format!("goodbye:{}", String::from_utf8_lossy(&seen)).as_bytes())
            .await?;
        stream.shutdown().await?;
        let _ = tx.send(seen);
        TestResult::Ok(())
    });
    Ok((port, got))
}

#[tokio::test]
async fn a_connect_client_that_ends_mid_decision_is_closed_at_once_and_its_verdict_dropped() -> TestResult {
    let upstream = echo_upstream().await?;
    let (mut h, verdict) = start_with_held_verdict(true, DENIED_ANSWER).await?;
    let mut stream = slow_connect(&h, upstream).await?;
    stream.write_all(b"ping").await?;
    decision_begun(&h).await?;
    stream.shutdown().await?;
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest))
        .await
        .map_err(|_| "the connection stayed open before the verdict")??;
    assert!(rest.is_empty(), "{:?}", String::from_utf8_lossy(&rest));
    assert!(h.outcomes.try_recv().is_err(), "the decision runs on");
    verdict.add_permits(1);
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::Io(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_connect_sent_with_its_end_spends_no_decision() -> TestResult {
    let upstream = echo_upstream().await?;
    let (mut h, _verdict) = start_with_held_verdict(true, IpAddr::V4(Ipv4Addr::LOCALHOST)).await?;
    let mut sent = std::net::TcpStream::connect(h.addr)?;
    let target = format!("allowed.test:{upstream}");
    std::io::Write::write_all(
        &mut sent,
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
            basic(TOKEN)
        )
        .as_bytes(),
    )?;
    sent.shutdown(std::net::Shutdown::Write)?;
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Rejected(IngressRejectionError::Io(_))
    ));
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_client_end_through_an_established_tunnel_reaches_the_upstream_as_a_clean_end() -> TestResult {
    for capture in [true, false] {
        let (upstream, mut got) = last_word_upstream().await?;
        let (h, verdict) = start_with_held_verdict(capture, IpAddr::V4(Ipv4Addr::LOCALHOST)).await?;
        let mut stream = slow_connect(&h, upstream).await?;
        verdict.add_permits(1);
        let head = read_head(&mut stream).await?;
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        stream.write_all(b"ping").await?;
        stream.shutdown().await?;
        assert_eq!(
            got.recv().await.ok_or("the outcome channel closed")?,
            b"ping",
            "capture {capture}"
        );
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await?;
        assert_eq!(reply, b"goodbye:ping", "capture {capture}");
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_client_that_ends_mid_decision_still_gets_its_verdict() -> TestResult {
    let h = {
        let (h, verdict) = start_with_held_verdict(false, DENIED_ANSWER).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(proxied("GET", 80, "/", "", "").as_bytes()).await?;
        decision_begun(&h).await?;
        stream.shutdown().await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        verdict.add_permits(1);
        let reply = drain(&mut stream).await?;
        assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
        h
    };
    let mut h = h;
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Denied(_),
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn a_plain_client_gone_mid_decision_leaves_the_proxy_serving() -> TestResult {
    let (mut h, verdict) = start_with_held_verdict(false, DENIED_ANSWER).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", 80, "/", "", "").as_bytes()).await?;
    decision_begun(&h).await?;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(50)).await;
    verdict.add_permits(1);
    assert!(matches!(
        h.outcomes.recv().await.ok_or("the outcome channel closed")?,
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Denied(_),
            ..
        }
    ));
    let mut next = TcpStream::connect(h.addr).await?;
    next.write_all(proxied("GET", 80, "/", "", "").as_bytes()).await?;
    verdict.add_permits(1);
    assert!(drain(&mut next).await?.starts_with("HTTP/1.1 403 "));
    Ok(())
}

async fn parent_port() -> TestResult<u16> {
    let parent = TcpListener::bind("127.0.0.1:0").await?;
    let port = parent.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = parent.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut head = [0u8; 1024];
                let Ok(n) = stream.read(&mut head).await else {
                    return;
                };
                if !head[..n].starts_with(b"CONNECT ") {
                    let _ = stream.write_all(OK_RESPONSE).await;
                    return;
                }
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                let mut buf = [0u8; 64];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    Ok(port)
}

async fn parent_proxy_options() -> TestResult<NetworkOptions> {
    let url = ParentProxyUrl::new(&format!("http://127.0.0.1:{}", parent_port().await?))?;
    let parent_proxy = ParentProxy::new(Some(url), None, NoProxy::new(""))?;
    Ok(NetworkOptions::default().with_parent_proxy(parent_proxy))
}

async fn start_resolving_to_metadata(options: NetworkOptions) -> TestResult<Harness> {
    start_configured(
        IngressLimits::default(),
        vec![metadata()],
        policy()?,
        Duration::ZERO,
        false,
        None,
        options,
    )
    .await
}

#[tokio::test]
async fn a_name_routed_to_a_parent_proxy_skips_the_lookup_and_the_floor_with_reasoned_refusals() -> TestResult {
    let options = parent_proxy_options().await?;
    let h = start_resolving_to_metadata(options.clone().with_reasoned_refusals()).await?;
    let (mut tunnel, head) = http_connect(h.addr, "allowed.test:443", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut tunnel).await?;
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);

    let h = start_resolving_to_metadata(options).await?;
    let (_s, head) = http_connect(h.addr, "allowed.test:443", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert!(h.resolver.calls.load(Ordering::SeqCst) > 0);
    Ok(())
}

#[tokio::test]
async fn a_name_routed_to_a_parent_proxy_is_not_floor_checked_over_socks_with_reasoned_refusals() -> TestResult {
    let options = parent_proxy_options().await?.with_reasoned_refusals();
    let h = start_resolving_to_metadata(options).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    assert_eq!(socks_handshake(&mut stream, TOKEN).await?, [1, 0]);
    let reply = socks_request(&mut stream, 1, 3, &domain_address("allowed.test"), 443).await?;
    assert_eq!(reply, 0);
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_mitm_name_skips_the_lookup_and_the_floor_with_reasoned_refusals() -> TestResult {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("m.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut head = [0u8; 512];
                let _ = stream.read(&mut head).await;
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                let mut buf = [0u8; 64];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let options = NetworkOptions::default().with_mitm(MitmUnixSocket {
        socket_path: socket,
        domains: vec![DomainPattern::new("allowed.test")?],
    });
    let h = start_resolving_to_metadata(options.clone().with_reasoned_refusals()).await?;
    let (mut tunnel, head) = http_connect(h.addr, "allowed.test:443", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut tunnel).await?;
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);

    let h = start_resolving_to_metadata(options).await?;
    let (_s, head) = http_connect(h.addr, "allowed.test:443", Some(&basic(TOKEN))).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn userinfo_in_a_plain_request_target_is_dropped_with_reasoned_refusals() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://user:secret@allowed.test:{port}/ HTTP/1.1\r\nHost: allowed.test:{port}\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let seen = lock(&seen)?;
    assert_eq!(seen.len(), 1);
    assert!(!String::from_utf8_lossy(&seen[0]).contains("secret"));
    Ok(())
}

#[tokio::test]
async fn userinfo_in_a_plain_request_target_is_refused_when_hosts_must_be_ascii() -> TestResult {
    let h = start_configured(
        IngressLimits::proxy_only(),
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        policy()?,
        Duration::ZERO,
        false,
        None,
        NetworkOptions::default().with_reasoned_refusals(),
    )
    .await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://user:secret@allowed.test:80/ HTTP/1.1\r\nHost: allowed.test\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 400 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn a_plain_407_keeps_the_connection_open_with_reasoned_refusals() -> TestResult {
    let (port, _seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let unauthenticated = format!("GET http://allowed.test:{port}/ HTTP/1.1\r\nHost: allowed.test:{port}\r\n\r\n");
    stream.write_all(unauthenticated.as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 407 "), "{head}");
    assert!(head.contains("Connection: keep-alive\r\n"), "{head}");
    assert!(!head.contains("Connection: close"), "{head}");
    let mut tail = [0u8; 5];
    stream.read_exact(&mut tail).await?;

    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    Ok(())
}

#[tokio::test]
async fn a_plain_407_still_closes_when_the_client_asks_for_it() -> TestResult {
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    for request in [
        "GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\nConnection: close\r\n\r\n",
        "GET http://allowed.test/ HTTP/1.0\r\n\r\n",
    ] {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        assert!(response.starts_with("HTTP/1.1 407 "), "{response}");
    }
    Ok(())
}

#[tokio::test]
async fn plain_http_is_routed_to_the_http_parent_proxy_only_with_reasoned_refusals() -> TestResult {
    let url = ParentProxyUrl::new(&format!("http://127.0.0.1:{}", parent_port().await?))?;
    let options = NetworkOptions::default().with_parent_proxy(ParentProxy::new(Some(url), None, NoProxy::new(""))?);
    let h = start_resolving_to_metadata(options.clone().with_reasoned_refusals()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", 80, "/", "", "").as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(h.resolver.calls.load(Ordering::SeqCst), 0);

    let h = start_resolving_to_metadata(options).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", 80, "/", "", "").as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn plain_http_ignores_a_parent_proxy_that_only_covers_https() -> TestResult {
    let url = ParentProxyUrl::new(&format!("http://127.0.0.1:{}", parent_port().await?))?;
    let options = NetworkOptions::default()
        .with_parent_proxy(ParentProxy::new(None, Some(url), NoProxy::new(""))?)
        .with_reasoned_refusals();
    let h = start_resolving_to_metadata(options).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", 80, "/", "", "").as_bytes()).await?;
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert!(h.resolver.calls.load(Ordering::SeqCst) > 0);
    Ok(())
}

#[tokio::test]
async fn an_unresolved_routed_name_reports_no_destination_address() -> TestResult {
    let options = parent_proxy_options().await?.with_reasoned_refusals();
    let mut h = start_resolving_to_metadata(options).await?;
    let (mut tunnel, _) = http_connect(h.addr, "allowed.test:443", Some(&basic(TOKEN))).await?;
    echo_roundtrip(&mut tunnel).await?;
    drop(tunnel);
    let outcome = h.outcomes.recv().await.ok_or("the outcome channel closed")?;
    assert!(
        matches!(outcome, IngressOutcome::Proxied { dest_ip: None, .. }),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_plain_407_is_worded_as_the_reference_server_words_it() -> TestResult {
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    let first = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"sandbox\"\r\n";
    for (request, tail) in [
        (
            "GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\n\r\n",
            "Connection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
        (
            "GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\nConnection: close\r\n\r\n",
            "Connection: close\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
        ("GET http://allowed.test/ HTTP/1.0\r\n\r\n", "Connection: close\r\n\r\n"),
    ] {
        let date_line = "Date: Mon, 26 Oct 2026 13:19:26 GMT\r\n";
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut reply = vec![0u8; first.len() + date_line.len() + tail.len()];
        stream.read_exact(&mut reply).await?;
        let reply = String::from_utf8(reply)?;
        let (status, rest) = reply.split_at(first.len());
        let (date, rest) = rest.split_at(date_line.len());
        assert_eq!(status, first, "{request}");
        assert!(date.starts_with("Date: ") && date.ends_with(" GMT\r\n"), "{date}");
        assert_eq!(rest, tail, "{request}");
    }
    Ok(())
}

#[tokio::test]
async fn a_plain_407_answers_an_expect_continue_like_the_reference_server_does() -> TestResult {
    let (port, _seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start_with_options(NetworkOptions::default().with_reasoned_refusals()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let unauthenticated = format!(
        "POST http://allowed.test:{port}/ HTTP/1.1\r\nHost: allowed.test:{port}\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\n"
    );
    stream.write_all(unauthenticated.as_bytes()).await?;
    assert_eq!(read_head(&mut stream).await?, "HTTP/1.1 100 Continue\r\n\r\n");
    let head = read_head(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 407 "), "{head}");
    let mut tail = [0u8; 5];
    stream.read_exact(&mut tail).await?;
    assert_eq!(&tail, b"0\r\n\r\n");

    stream.write_all(b"hello").await?;
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    Ok(())
}

#[tokio::test]
async fn an_idle_connection_after_a_plain_407_closes_after_the_reference_keep_alive_timeout_and_buffer() -> TestResult {
    let limits = IngressLimits {
        keep_alive_timeout: Duration::from_millis(300),
        ..IngressLimits::default()
    };
    let h = start_configured(
        limits,
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        policy()?,
        Duration::ZERO,
        false,
        None,
        NetworkOptions::default().with_reasoned_refusals(),
    )
    .await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let before_answer = std::time::Instant::now();
    stream
        .write_all(b"GET http://allowed.test/ HTTP/1.1\r\nHost: allowed.test\r\n\r\n")
        .await?;
    let _ = read_head(&mut stream).await?;
    let mut rest = [0u8; 5];
    stream.read_exact(&mut rest).await?;
    let after_answer = std::time::Instant::now();
    let mut end = Vec::new();
    stream.read_to_end(&mut end).await?;
    let at_most = after_answer.elapsed();
    let at_least = before_answer.elapsed();
    assert!(at_least >= Duration::from_millis(1300), "{at_least:?}");
    assert!(at_most < Duration::from_millis(2500), "{at_most:?}");
    Ok(())
}

async fn named_upstream_head(raw: String) -> TestResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let head = tokio::spawn(async move {
        let (mut probe, _) = listener.accept().await?;
        let mut probed = Vec::new();
        probe.read_to_end(&mut probed).await?;
        assert!(probed.is_empty(), "{probed:?}");
        let (mut stream, _) = listener.accept().await?;
        let head = read_head(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await?;
        TestResult::Ok(head)
    });
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let raw = raw.replace("{port}", &port.to_string());
    stream.write_all(raw.as_bytes()).await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    head.await?
}

#[tokio::test]
async fn plain_http_folds_header_names_and_duplicates_as_the_reference_server_does() -> TestResult {
    let raw = format!(
        "GET http://allowed.test:{{port}}/p?q=1 HTTP/1.1\r\nX-First: 1\r\nUser-Agent: UA\r\nUser-Agent: other\r\nHost: allowed.test:{{port}}\r\nAccept: */*\r\nX-Dup: a\r\nX-Mixed-Case: v\r\nx-dup: b\r\nCookie: a=1\r\nCookie: b=2\r\nSet-Cookie: s=1\r\nSet-Cookie: t=2\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    let saw = named_upstream_head(raw).await?;
    let port = saw
        .lines()
        .find_map(|l| l.strip_prefix("host: allowed.test:"))
        .ok_or("no host line")?
        .to_string();
    assert_eq!(
        saw,
        format!(
            "GET /p?q=1 HTTP/1.1\r\nx-first: 1\r\nuser-agent: UA\r\nhost: allowed.test:{port}\r\naccept: */*\r\nx-dup: a, b\r\nx-mixed-case: v\r\ncookie: a=1; b=2\r\nset-cookie: s=1\r\nset-cookie: t=2\r\nConnection: close\r\n\r\n"
        )
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_to_an_ip_literal_dials_once() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let head = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let head = read_head(&mut stream).await?;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await?;
        TestResult::Ok(head)
    });
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let request = format!(
        "GET http://127.0.0.1:{port}/p HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nProxy-Authorization: {}\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(
        head.await??,
        format!("GET /p HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
    );
    Ok(())
}

#[tokio::test]
async fn plain_http_keeps_the_client_connection_open_when_upstream_closes() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut probe, _)) = listener.accept().await else {
                break;
            };
            let mut probed = Vec::new();
            probe.read_to_end(&mut probed).await?;
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            read_head(&mut stream).await?;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await?;
        }
        TestResult::Ok(())
    });
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    for _ in 0..2 {
        let request = proxied("GET", port, "/p", "", "");
        stream.write_all(request.as_bytes()).await?;
        let head = read_head(&mut stream).await?;
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        let mut body = [0u8; 2];
        stream.read_exact(&mut body).await?;
        assert_eq!(&body, b"ok");
    }
    Ok(())
}

/// One connection a parent proxy accepted: the requests it carried, when it
/// last answered one, and when the proxy closed it.
#[derive(Debug, Clone, Default)]
struct ParentConnection {
    requests: usize,
    answered: Option<Instant>,
    closed: Option<Instant>,
}

type ParentLog = Arc<Mutex<Vec<ParentConnection>>>;

async fn next_head(stream: &mut TcpStream) -> Option<String> {
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

/// Whether the recording parent closes a connection after an answer that
/// says `Connection: close`, or keeps it open whatever it said.
#[derive(Clone, Copy)]
enum ParentCloses {
    AsItSays,
    Never,
}

/// A parent proxy that answers each request with `extra` in its head, holding
/// every answer until `hold` requests have come in.
fn note(log: &ParentLog, index: usize, update: impl FnOnce(&mut ParentConnection)) {
    if let Ok(mut connections) = log.lock()
        && let Some(connection) = connections.get_mut(index)
    {
        update(connection);
    }
}

async fn recording_parent(extra: &'static str, hold: usize, closes: ParentCloses) -> TestResult<(u16, ParentLog)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let log = ParentLog::default();
    let recorded = Arc::clone(&log);
    let (arrived, _) = tokio::sync::watch::channel(0usize);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            let arrived = arrived.clone();
            tokio::spawn(async move {
                let index = {
                    let Ok(mut log) = recorded.lock() else {
                        return;
                    };
                    log.push(ParentConnection::default());
                    log.len() - 1
                };
                let mut gate = arrived.subscribe();
                while next_head(&mut stream).await.is_some() {
                    note(&recorded, index, |connection| connection.requests += 1);
                    arrived.send_modify(|count| *count += 1);
                    let _ = gate.wait_for(|count| *count >= hold).await;
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n{extra}\r\nok");
                    if stream.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                    note(&recorded, index, |connection| {
                        connection.answered = Some(Instant::now());
                    });
                    if matches!(closes, ParentCloses::AsItSays) && extra.contains("close") {
                        return;
                    }
                }
                note(&recorded, index, |connection| {
                    connection.closed = Some(Instant::now());
                });
            });
        }
    });
    Ok((port, log))
}

async fn behind_recording_parent(extra: &'static str, hold: usize) -> TestResult<(Harness, ParentLog)> {
    behind_parent_stripping(extra, hold, ParentCloses::AsItSays, ResponseHeaderStrip::default()).await
}

async fn behind_parent_stripping(
    extra: &'static str,
    hold: usize,
    closes: ParentCloses,
    strip: ResponseHeaderStrip,
) -> TestResult<(Harness, ParentLog)> {
    let (port, log) = recording_parent(extra, hold, closes).await?;
    let url = ParentProxyUrl::new(&format!("http://127.0.0.1:{port}"))?;
    let options = NetworkOptions::default()
        .with_parent_proxy(ParentProxy::new(Some(url), None, NoProxy::new(""))?)
        .with_strip_response_headers(strip)
        .with_reasoned_refusals();
    Ok((start_resolving_to_metadata(options).await?, log))
}

fn stripping(name: &str) -> TestResult<ResponseHeaderStrip> {
    Ok(ResponseHeaderStrip::new([StrippedHeader::new(name)?]))
}

async fn get_through(stream: &mut TcpStream, path: &str) -> TestResult {
    stream.write_all(proxied("GET", 80, path, "", "").as_bytes()).await?;
    let (head, body) = read_response(stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(body, "ok");
    Ok(())
}

/// What the parent saw, once the proxy has had time to free the connection
/// its last answer came on.
async fn settled(log: &ParentLog) -> TestResult<Vec<ParentConnection>> {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(lock(log)?.clone())
}

async fn closed(log: &ParentLog, index: usize) -> TestResult<ParentConnection> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let connection = lock(log)?
                .get(index)
                .cloned()
                .ok_or("the parent saw no such connection")?;
            if connection.closed.is_some() {
                return TestResult::Ok(connection);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "the proxy kept the parent connection open")?
}

fn free_for(connection: &ParentConnection) -> TestResult<Duration> {
    let closed = connection.closed.ok_or("the connection never closed")?;
    let answered = connection.answered.ok_or("the connection never answered")?;
    Ok(closed - answered)
}

#[tokio::test]
async fn plain_requests_from_every_client_share_a_freed_parent_connection() -> TestResult {
    let (h, log) = behind_recording_parent("", 1).await?;
    let mut first = TcpStream::connect(h.addr).await?;
    get_through(&mut first, "/a").await?;
    get_through(&mut first, "/b").await?;
    settled(&log).await?;
    let mut second = TcpStream::connect(h.addr).await?;
    get_through(&mut second, "/c").await?;
    let seen = settled(&log).await?;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].requests, 3);
    assert!(seen[0].closed.is_none(), "{seen:?}");
    Ok(())
}

#[tokio::test]
async fn concurrent_plain_requests_each_open_a_parent_connection_that_a_later_one_reuses() -> TestResult {
    let (h, log) = behind_recording_parent("", 3).await?;
    let mut clients = Vec::new();
    for _ in 0..3 {
        clients.push(TcpStream::connect(h.addr).await?);
    }
    let mut pending = tokio::task::JoinSet::new();
    for (index, mut client) in clients.into_iter().enumerate() {
        pending.spawn(async move {
            get_through(&mut client, &format!("/{index}")).await?;
            TestResult::Ok(client)
        });
    }
    let clients = pending.join_all().await.into_iter().collect::<TestResult<Vec<_>>>()?;
    settled(&log).await?;
    let mut later = TcpStream::connect(h.addr).await?;
    get_through(&mut later, "/later").await?;
    let seen = settled(&log).await?;
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(seen.iter().map(|c| c.requests).sum::<usize>(), 4);
    drop(clients);
    Ok(())
}

#[tokio::test]
async fn a_parent_connection_left_free_for_five_seconds_is_closed() -> TestResult {
    let (h, log) = behind_recording_parent("", 1).await?;
    let mut client = TcpStream::connect(h.addr).await?;
    get_through(&mut client, "/a").await?;
    let connection = closed(&log, 0).await?;
    let free = free_for(&connection)?;
    assert!(
        (Duration::from_millis(4800)..Duration::from_millis(5800)).contains(&free),
        "{free:?}"
    );
    get_through(&mut client, "/b").await?;
    assert_eq!(settled(&log).await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_keep_alive_timeout_frees_a_parent_connection_for_a_second_less() -> TestResult {
    let (h, log) = behind_recording_parent("Keep-Alive: timeout=2\r\n", 1).await?;
    let mut client = TcpStream::connect(h.addr).await?;
    get_through(&mut client, "/a").await?;
    let free = free_for(&closed(&log, 0).await?)?;
    assert!(
        (Duration::from_millis(800)..Duration::from_millis(1800)).contains(&free),
        "{free:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_keep_alive_timeout_of_a_second_keeps_no_parent_connection() -> TestResult {
    let (h, log) = behind_recording_parent("Keep-Alive: timeout=1\r\n", 1).await?;
    let mut client = TcpStream::connect(h.addr).await?;
    get_through(&mut client, "/a").await?;
    let free = free_for(&closed(&log, 0).await?)?;
    assert!(free < Duration::from_millis(500), "{free:?}");
    get_through(&mut client, "/b").await?;
    assert_eq!(settled(&log).await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_parent_that_closes_its_connections_gets_one_per_request() -> TestResult {
    let (h, log) = behind_recording_parent("Connection: close\r\n", 1).await?;
    for path in ["/a", "/b", "/c"] {
        let mut client = TcpStream::connect(h.addr).await?;
        get_through(&mut client, path).await?;
    }
    let seen = settled(&log).await?;
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen.iter().all(|c| c.requests == 1), "{seen:?}");
    Ok(())
}

#[tokio::test]
async fn a_keep_alive_timeout_is_read_before_the_response_strip_drops_it() -> TestResult {
    let (h, log) = behind_parent_stripping(
        "Keep-Alive: timeout=2\r\n",
        1,
        ParentCloses::Never,
        stripping("keep-alive")?,
    )
    .await?;
    let mut client = TcpStream::connect(h.addr).await?;
    get_through(&mut client, "/a").await?;
    let free = free_for(&closed(&log, 0).await?)?;
    assert!(
        (Duration::from_millis(800)..Duration::from_millis(1800)).contains(&free),
        "{free:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_parent_connection_close_is_read_before_the_response_strip_drops_it() -> TestResult {
    let (h, log) = behind_parent_stripping(
        "Connection: close\r\n",
        1,
        ParentCloses::Never,
        stripping("connection")?,
    )
    .await?;
    let mut client = TcpStream::connect(h.addr).await?;
    get_through(&mut client, "/a").await?;
    settled(&log).await?;
    get_through(&mut client, "/b").await?;
    let seen = settled(&log).await?;
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen.iter().all(|c| c.requests == 1), "{seen:?}");
    Ok(())
}

#[path = "ingress_mux/reference_server_refusals_tests.rs"]
mod reference_server_refusals_tests;
