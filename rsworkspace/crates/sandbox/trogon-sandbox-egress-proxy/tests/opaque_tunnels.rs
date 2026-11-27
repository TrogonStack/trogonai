use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{
    DomainRule, DomainRuleAction, EgressViolation, EgressViolationReason, ForwardUrl, IpRange, LeaseId, NetworkPolicy,
    OpaqueTunnelPolicy, RequestForward,
};
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;
use trogon_sandbox_egress_proxy::ingress::{Ingress, IngressLimits, IngressOutcome, ProxyToken};
use trogon_sandbox_egress_proxy::oidc_signer::OidcSigner;
use trogon_sandbox_egress_proxy::proxy::{OidcIdentityContext, ProxyOutcome, TlsInterceptContext};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};

const TOKEN: &str = "lease-token-value";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct Loopback;

#[async_trait]
impl NameLookup for Loopback {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
    }
}

#[derive(Default)]
struct CollectingSink(Mutex<Vec<Vec<u8>>>);

#[async_trait]
impl ViolationSink for CollectingSink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        locked(&self.0).push(payload);
        Ok(())
    }
}

fn rules() -> TestResult<Vec<DomainRule>> {
    Ok(vec![
        DomainRule {
            domain: "plain.test".parse()?,
            action: DomainRuleAction::PassThrough,
        },
        DomainRule {
            domain: "terminated.test".parse()?,
            action: DomainRuleAction::Forward(RequestForward {
                forward_url: ForwardUrl::new("https://forward.test")?,
                match_rules: vec![],
                ttl_seconds: RequestForward::DEFAULT_TTL_SECONDS,
            }),
        },
    ])
}

fn policy(opaque_tunnels: OpaqueTunnelPolicy) -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: rules()?,
        allow_ranges: vec![IpRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels,
        request_rules: vec![],
    })
}

fn tls_context() -> TestResult<TlsInterceptContext> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(PermittedDomains::from_rules(&rules()?)?)?;
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
    sink: Arc<CollectingSink>,
}

async fn start(opaque_tunnels: OpaqueTunnelPolicy) -> TestResult<Harness> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let policy = Arc::new(policy(opaque_tunnels)?);
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1".to_string()),
        sink.clone(),
        ViolationRateLimit::default(),
    ));
    let ingress = Arc::new(Ingress::new(
        ProxyToken::new(TOKEN)?,
        Arc::new(VettingResolver::new(Arc::new(Loopback), SsrfFloor::system())),
        IngressLimits::default(),
    ));
    let tls = Arc::new(tls_context()?);
    let (tx, outcomes) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let ingress = ingress.clone();
            let policy = policy.clone();
            let tls = tls.clone();
            let reporter = reporter.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let outcome = ingress.serve(stream, &policy, &tls).await;
                reporter.report_ingress(&outcome).await;
                let _ = tx.send(outcome);
            });
        }
    });
    Ok(Harness { addr, outcomes, sink })
}

async fn echo_upstream() -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
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
    let auth = STANDARD.encode(format!("sandbox:{TOKEN}"));
    stream
        .write_all(
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let head = read_head(&mut stream).await?;
    Ok((stream, head))
}

async fn socks_connect(proxy: SocketAddr, name: &str, port: u16) -> TestResult<(TcpStream, u8)> {
    let mut stream = TcpStream::connect(proxy).await?;
    stream.write_all(&[5, 1, 2]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    let user = b"sandbox";
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user);
    auth.push(TOKEN.len() as u8);
    auth.extend_from_slice(TOKEN.as_bytes());
    stream.write_all(&auth).await?;
    let mut status = [0u8; 2];
    stream.read_exact(&mut status).await?;
    let mut request = vec![5, 1, 0, 3, name.len() as u8];
    request.extend_from_slice(name.as_bytes());
    request.extend(port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await?;
    Ok((stream, reply[1]))
}

async fn assert_denied_as_opaque_tunnel(h: &mut Harness) -> TestResult {
    match h.outcomes.recv().await.ok_or("no ingress outcome")? {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::Denied(denial),
            ..
        } => assert_eq!(denial.reason, EgressViolationReason::OpaqueTunnelRefused),
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

async fn assert_refused_and_recorded(h: &mut Harness, host: &str) -> TestResult {
    assert_denied_as_opaque_tunnel(h).await?;
    let events = locked(&h.sink.0);
    assert_eq!(events.len(), 1);
    let event: EgressViolation = serde_json::from_slice(&events[0])?;
    assert_eq!(event.host.as_deref(), Some(host));
    assert_eq!(event.reason, EgressViolationReason::OpaqueTunnelRefused);
    Ok(())
}

#[tokio::test]
async fn an_unterminated_connect_is_refused_and_recorded_when_the_switch_is_on() -> TestResult {
    let upstream = echo_upstream().await?;
    let mut h = start(OpaqueTunnelPolicy::Refuse).await?;
    let (_stream, head) = http_connect(h.addr, &format!("plain.test:{upstream}")).await?;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    assert_refused_and_recorded(&mut h, "plain.test").await?;
    Ok(())
}

#[tokio::test]
async fn an_unterminated_connect_is_tunnelled_when_the_switch_is_off() -> TestResult {
    let upstream = echo_upstream().await?;
    let h = start(OpaqueTunnelPolicy::Allow).await?;
    let (mut stream, head) = http_connect(h.addr, &format!("plain.test:{upstream}")).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    stream.write_all(b"ping").await?;
    let mut got = [0u8; 4];
    stream.read_exact(&mut got).await?;
    assert_eq!(&got, b"ping");
    Ok(())
}

async fn assert_socks_refused_and_recorded(h: &mut Harness, host: &str, port: u16) -> TestResult {
    assert_denied_as_opaque_tunnel(h).await?;
    let events = locked(&h.sink.0);
    assert_eq!(events.len(), 1);
    let event: EgressViolation = serde_json::from_slice(&events[0])?;
    assert_eq!(event.host.as_deref(), Some(host));
    let request = event.request.ok_or("the refused SOCKS CONNECT")?;
    assert_eq!(request.method(), "SOCKS");
    assert_eq!(request.target(), format!("{host}:{port}"));
    assert_eq!(request.reason(), "refuseOpaqueTunnels");
    Ok(())
}

#[tokio::test]
async fn every_socks_connect_is_refused_with_a_general_failure_when_the_switch_is_on() -> TestResult {
    let upstream = echo_upstream().await?;
    for host in ["plain.test", "terminated.test", "unlisted.test"] {
        let mut h = start(OpaqueTunnelPolicy::Refuse).await?;
        let (_stream, reply) = socks_connect(h.addr, host, upstream).await?;
        assert_eq!(reply, 0x01, "{host}");
        assert_socks_refused_and_recorded(&mut h, host, upstream).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_terminated_host_is_carried_unread_over_socks() -> TestResult {
    let upstream = echo_upstream().await?;
    let h = start(OpaqueTunnelPolicy::Allow).await?;
    let (mut stream, reply) = socks_connect(h.addr, "terminated.test", upstream).await?;
    assert_eq!(reply, 0x00);
    stream.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").await?;
    let mut got = [0u8; 21];
    stream.read_exact(&mut got).await?;
    assert_eq!(&got, b"SSH-2.0-OpenSSH_9.6\r\n");
    Ok(())
}

#[tokio::test]
async fn non_tls_bytes_into_a_terminated_tunnel_are_refused_and_recorded() -> TestResult {
    let mut h = start(OpaqueTunnelPolicy::Refuse).await?;
    let (mut stream, head) = http_connect(h.addr, "terminated.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    stream.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").await?;
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await?;
    assert!(rest.is_empty());
    assert_refused_and_recorded(&mut h, "terminated.test").await?;
    Ok(())
}
