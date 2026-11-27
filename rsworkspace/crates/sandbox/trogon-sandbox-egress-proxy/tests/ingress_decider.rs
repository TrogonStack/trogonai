use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{
    AllowRange, DomainRule, DomainRuleAction, EgressViolation, EgressViolationReason, IpRange, LeaseId, NetworkPolicy,
    SandboxId, TenantId,
};
use trogon_sandbox_contract::{NetworkOptions, UnlistedDestinations};
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;
use trogon_sandbox_egress_proxy::decider::{
    Decider, DeciderLimits, DeciderTransport, DeciderTransportError, LivePolicyAmender, PolicyPublisher,
};
use trogon_sandbox_egress_proxy::ingress::{Ingress, IngressLimits, ProxyToken};
use trogon_sandbox_egress_proxy::oidc_signer::OidcSigner;
use trogon_sandbox_egress_proxy::proxy::{OidcIdentityContext, TlsInterceptContext};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const TOKEN: &str = "lease-token-value";

struct StubLookup {
    answer: IpAddr,
}

#[async_trait]
impl NameLookup for StubLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(vec![self.answer])
    }
}

#[derive(Default)]
struct CollectingSink(Mutex<Vec<Vec<u8>>>);

#[async_trait]
impl ViolationSink for CollectingSink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        self.0
            .lock()
            .map_err(|_| ViolationPublishError("sink poisoned".to_string()))?
            .push(payload);
        Ok(())
    }
}

enum Reply {
    Bytes(&'static str),
    Unavailable,
    Hang,
}

struct ScriptedTransport {
    reply: Reply,
    count: AtomicUsize,
}

impl ScriptedTransport {
    fn new(reply: Reply) -> Arc<Self> {
        Arc::new(Self {
            reply,
            count: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl DeciderTransport for ScriptedTransport {
    async fn request(&self, _subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        if serde_json::from_slice::<serde_json::Value>(&payload).is_err() {
            return Err(DeciderTransportError::Unavailable("request was not JSON".into()));
        }
        match &self.reply {
            Reply::Bytes(bytes) => Ok(bytes.as_bytes().to_vec()),
            Reply::Unavailable => Err(DeciderTransportError::Unavailable("no responders".into())),
            Reply::Hang => std::future::pending().await,
        }
    }
}

#[derive(Default)]
struct RecordingPublisher(Mutex<Vec<NetworkPolicy>>);

#[async_trait]
impl PolicyPublisher for RecordingPublisher {
    async fn publish(&self, _lease: &LeaseId, policy: &NetworkPolicy) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "publisher poisoned".to_string())?
            .push(policy.clone());
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
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn policy_without_ranges() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec!["blocked.test".parse()?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
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
        OidcIdentityContext {
            signer: OidcSigner::generate()?,
            issuer: "test".to_string(),
            subject: SandboxId::new("test")?,
            tenant_id: TenantId::new("t")?,
            lease_id: LeaseId::new("l"),
        },
    ))
}

struct Harness {
    addr: SocketAddr,
    sink: Arc<CollectingSink>,
    transport: Arc<ScriptedTransport>,
    policy: Arc<RwLock<NetworkPolicy>>,
    publisher: Arc<RecordingPublisher>,
}

struct Setup {
    reply: Option<Reply>,
    answer: IpAddr,
    policy: NetworkPolicy,
    timeout: Duration,
    options: NetworkOptions,
}

impl Setup {
    fn replying(bytes: &'static str) -> TestResult<Self> {
        Self::with(Some(Reply::Bytes(bytes)))
    }

    fn with(reply: Option<Reply>) -> TestResult<Self> {
        Ok(Self {
            reply,
            answer: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)),
            policy: policy_without_ranges()?,
            timeout: Duration::from_secs(5),
            options: NetworkOptions::default(),
        })
    }

    fn resolving_to(mut self, answer: IpAddr) -> Self {
        self.answer = answer;
        self
    }

    fn strict(mut self) -> Self {
        self.options.unlisted = UnlistedDestinations::Refuse;
        self
    }
}

async fn start(setup: Setup) -> TestResult<Harness> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink.clone(),
        ViolationRateLimit::default(),
    ));
    let transport = ScriptedTransport::new(setup.reply.map_or(Reply::Unavailable, |reply| reply));
    let publisher = Arc::new(RecordingPublisher::default());
    let shared: Arc<RwLock<NetworkPolicy>> = Arc::new(RwLock::new(setup.policy));
    let mut ingress = Ingress::new(
        ProxyToken::new(TOKEN)?,
        Arc::new(VettingResolver::new(
            Arc::new(StubLookup { answer: setup.answer }),
            SsrfFloor::system(),
        )),
        IngressLimits::default(),
    );
    let decider = Arc::new(Decider::new(
        LeaseId::new("L1"),
        transport.clone(),
        Arc::new(LivePolicyAmender::new(
            LeaseId::new("L1"),
            shared.clone(),
            publisher.clone(),
        )),
        DeciderLimits {
            timeout: setup.timeout,
            max_in_flight: 4,
        },
    ));
    ingress = ingress
        .with_decider(decider)
        .with_network_options(Arc::new(setup.options));
    let ingress = Arc::new(ingress);
    let tls = Arc::new(tls_context()?);
    let served = shared.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let ingress = ingress.clone();
            let tls = tls.clone();
            let reporter = reporter.clone();
            let served = served.clone();
            tokio::spawn(async move {
                let snapshot = served.read().await.clone();
                let outcome = ingress.serve(stream, &snapshot, &tls).await;
                reporter.report_ingress(&outcome).await;
            });
        }
    });
    Ok(Harness {
        addr,
        sink,
        transport,
        policy: shared,
        publisher,
    })
}

async fn start_without_decider(policy: NetworkPolicy) -> TestResult<(SocketAddr, Arc<CollectingSink>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink.clone(),
        ViolationRateLimit::default(),
    ));
    let ingress = Arc::new(Ingress::new(
        ProxyToken::new(TOKEN)?,
        Arc::new(VettingResolver::new(
            Arc::new(StubLookup {
                answer: IpAddr::V4(Ipv4Addr::LOCALHOST),
            }),
            SsrfFloor::system(),
        )),
        IngressLimits::default(),
    ));
    let tls = Arc::new(tls_context()?);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let ingress = ingress.clone();
            let tls = tls.clone();
            let reporter = reporter.clone();
            let policy = policy.clone();
            tokio::spawn(async move {
                let outcome = ingress.serve(stream, &policy, &tls).await;
                reporter.report_ingress(&outcome).await;
            });
        }
    });
    Ok((addr, sink))
}

fn basic(password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{password}")))
}

async fn read_head(stream: &mut TcpStream) -> TestResult<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return Err(format!("closed early: {:?}", String::from_utf8_lossy(&buf)).into());
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8(buf)?)
}

async fn http_connect(proxy: SocketAddr, target: &str) -> TestResult<(TcpStream, String)> {
    let mut stream = TcpStream::connect(proxy).await?;
    let auth = basic(TOKEN);
    stream
        .write_all(
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {auth}\r\n\r\n").as_bytes(),
        )
        .await?;
    let head = read_head(&mut stream).await?;
    Ok((stream, head))
}

async fn violations(sink: &CollectingSink, expected: usize) -> TestResult<Vec<EgressViolation>> {
    for _ in 0..200 {
        {
            let events = sink.0.lock().map_err(|_| "sink poisoned")?;
            if events.len() >= expected {
                return Ok(events
                    .iter()
                    .map(|raw| serde_json::from_slice(raw))
                    .collect::<Result<_, _>>()?);
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!("expected {expected} violation events").into())
}

fn published(publisher: &RecordingPublisher) -> TestResult<usize> {
    Ok(publisher.0.lock().map_err(|_| "publisher poisoned")?.len())
}

const ALLOW_ONCE: &str = r#"{"verdict":"allow","scope":"once"}"#;
const ALLOW_PERSIST: &str = r#"{"verdict":"allow","scope":"persist"}"#;
const DENY_ONCE: &str = r#"{"verdict":"deny","scope":"once"}"#;

#[tokio::test]
async fn an_unlisted_host_the_decider_denies_gets_403_and_a_typed_violation() -> TestResult {
    let h = start(Setup::replying(DENY_ONCE)?).await?;
    let (_stream, head) = http_connect(h.addr, "unlisted.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    let events = violations(&h.sink, 1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::DeciderDenied);
    assert_eq!(events[0].host.as_deref(), Some("unlisted.test"));
    Ok(())
}

#[tokio::test]
async fn a_strict_allowlist_refuses_an_unlisted_host_without_asking_the_decider() -> TestResult {
    let h = start(Setup::replying(ALLOW_ONCE)?.strict()).await?;
    let (_stream, head) = http_connect(h.addr, "unlisted.test:443").await?;
    assert!(
        head.starts_with("HTTP/1.1 403 host is not on the allow list\r\n"),
        "{head}"
    );
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 0);
    let events = violations(&h.sink, 1).await?;
    assert_eq!(events[0].host.as_deref(), Some("unlisted.test"));
    Ok(())
}

#[tokio::test]
async fn a_strict_allowlist_still_lets_a_listed_host_through() -> TestResult {
    let h = start(
        Setup::replying(DENY_ONCE)?
            .strict()
            .resolving_to(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    )
    .await?;
    let mut policy = h.policy.write().await;
    *policy = self::policy()?;
    drop(policy);
    let (_stream, head) = http_connect(h.addr, "allowed.test:1").await?;
    assert!(!head.starts_with("HTTP/1.1 403 host is not on"), "{head}");
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn without_a_decider_an_unlisted_host_is_denied_as_before() -> TestResult {
    let (addr, sink) = start_without_decider(policy_without_ranges()?).await?;
    let (_stream, head) = http_connect(addr, "unlisted.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    let events = violations(&sink, 1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::PolicyDenied);
    Ok(())
}

#[tokio::test]
async fn a_denied_domain_never_reaches_the_decider() -> TestResult {
    let h = start(Setup::replying(ALLOW_ONCE)?).await?;
    let (_stream, head) = http_connect(h.addr, "blocked.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 0);
    let events = violations(&h.sink, 1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::PolicyDenied);
    Ok(())
}

#[tokio::test]
async fn a_deny_range_literal_never_reaches_the_decider() -> TestResult {
    let h = start(Setup::replying(ALLOW_ONCE)?).await?;
    let (_stream, head) = http_connect(h.addr, "10.1.2.3:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn the_ssrf_floor_wins_over_a_decider_that_would_allow() -> TestResult {
    let h = start(Setup::replying(ALLOW_PERSIST)?).await?;
    let (_stream, head) = http_connect(h.addr, "169.254.169.254:80").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 0);
    let events = violations(&h.sink, 1).await?;
    assert!(matches!(
        events[0].reason,
        EgressViolationReason::DestinationAddressDenied(_)
    ));
    assert_eq!(published(&h.publisher)?, 0);
    Ok(())
}

#[tokio::test]
async fn a_name_resolving_to_metadata_is_refused_without_asking() -> TestResult {
    let mut setup = Setup::replying(ALLOW_PERSIST)?;
    setup.policy = policy()?;
    setup.answer = IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254));
    let h = start(setup).await?;
    let (_stream, head) = http_connect(h.addr, "unlisted.test:80").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_persisted_deny_is_written_to_the_live_policy() -> TestResult {
    let setup = Setup::replying(r#"{"verdict":"deny","scope":"persist"}"#)?;
    let h = start(setup).await?;
    let (_stream, head) = http_connect(h.addr, "nope.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    for _ in 0..200 {
        if published(&h.publisher)? != 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let NetworkPolicy::UserDefined { denied_domains, .. } = h.policy.read().await.clone() else {
        return Err("policy changed shape".into());
    };
    assert_eq!(denied_domains.len(), 2);
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 1);

    let (_again, head) = http_connect(h.addr, "nope.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(h.transport.count.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_malformed_reply_denies_with_a_typed_reason() -> TestResult {
    let h = start(Setup::replying(r#"{"verdict":"maybe"}"#)?).await?;
    let (_stream, head) = http_connect(h.addr, "unlisted.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    let events = violations(&h.sink, 1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::DeciderMalformedReply);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_decider_denies_with_a_typed_reason() -> TestResult {
    let h = start(Setup::with(Some(Reply::Unavailable))?).await?;
    let (_stream, head) = http_connect(h.addr, "unlisted.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    let events = violations(&h.sink, 1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::DeciderUnavailable);
    Ok(())
}

#[tokio::test]
async fn a_silent_decider_times_out_into_a_typed_deny() -> TestResult {
    let mut setup = Setup::with(Some(Reply::Hang))?;
    setup.timeout = Duration::from_millis(100);
    let h = start(setup).await?;
    let (_stream, head) = http_connect(h.addr, "unlisted.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    let events = violations(&h.sink, 1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::DeciderTimedOut);
    Ok(())
}
