use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{
    DomainRule, DomainRuleAction, ForwardUrl, IpRange, LeaseId, NetworkPolicy, OpaqueTunnelPolicy, RequestForward,
    SandboxId, TenantId,
};
use trogon_sandbox_contract::{NetworkOptions, TunnelCap};
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;
use trogon_sandbox_egress_proxy::ingress::{Ingress, IngressLimits, IngressOutcome, ProxyToken};
use trogon_sandbox_egress_proxy::oidc_signer::OidcSigner;
use trogon_sandbox_egress_proxy::proxy::{OidcIdentityContext, ProxyOutcome, TlsInterceptContext};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};
use trogon_sandbox_egress_proxy::{TlsInterceptError, TunnelDeadlines};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const TOKEN: &str = "lease-token-value";

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
        self.0
            .lock()
            .map_err(|_| ViolationPublishError("sink poisoned".to_string()))?
            .push(payload);
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

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: rules()?,
        allow_ranges: vec![IpRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
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
    outcomes: mpsc::UnboundedReceiver<IngressOutcome>,
}

async fn start(limits: IngressLimits, deadlines: TunnelDeadlines) -> TestResult<Harness> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let options = NetworkOptions {
        tunnel_cap: Some(TunnelCap::new(1)?),
        ..NetworkOptions::default()
    };
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink,
        ViolationRateLimit::default(),
    ));
    let ingress = Arc::new(
        Ingress::new(
            ProxyToken::new(TOKEN)?,
            Arc::new(VettingResolver::new(Arc::new(Loopback), SsrfFloor::system())),
            limits,
        )
        .with_network_options(Arc::new(options)),
    );
    let tls = Arc::new(tls_context()?.with_deadlines(deadlines));
    let policy = policy()?;
    let (tx, outcomes) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let policy = policy.clone();
            let ingress = ingress.clone();
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
    Ok(Harness { addr, outcomes })
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

#[tokio::test]
async fn a_connect_that_never_sends_a_client_hello_is_closed_and_frees_its_slot() -> TestResult {
    let limits = IngressLimits::default();
    let deadlines = TunnelDeadlines {
        handshake: Duration::from_millis(300),
        ..TunnelDeadlines::default()
    };
    let mut h = start(limits, deadlines).await?;

    let (mut silent, head) = http_connect(h.addr, "terminated.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");

    let (mut other, head) = http_connect(h.addr, "terminated.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 503 "), "{head}");
    assert!(head.contains("X-Proxy-Error: too-many-tunnels"));
    drop(other.shutdown().await);

    let mut rest = Vec::new();
    timeout(Duration::from_secs(10), silent.read_to_end(&mut rest)).await??;
    assert!(rest.is_empty());

    let timed_out = loop {
        match h.outcomes.recv().await.ok_or("expected a value")? {
            IngressOutcome::Rejected(_) => continue,
            IngressOutcome::Proxied {
                outcome: ProxyOutcome::Denied(_),
                ..
            } => continue,
            proxied => break proxied,
        }
    };
    match timed_out {
        IngressOutcome::Proxied {
            outcome: ProxyOutcome::TlsInterceptFailed(TlsInterceptError::ClientHandshakeTimedOut { host, port, wait }),
            ..
        } => {
            assert_eq!((host.as_str(), port), ("terminated.test", 443));
            assert_eq!(wait, Duration::from_millis(300));
        }
        other => return Err(format!("{other:?}").into()),
    }

    let (_again, head) = http_connect(h.addr, "terminated.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    Ok(())
}

#[tokio::test]
async fn a_client_that_ends_short_of_a_record_header_is_closed_not_relayed() -> TestResult {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let h = start(IngressLimits::default(), TunnelDeadlines::default()).await?;

    let (mut client, head) = http_connect(h.addr, &format!("terminated.test:{port}")).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    client.write_all(b"G").await?;
    client.shutdown().await?;

    let mut rest = Vec::new();
    timeout(Duration::from_secs(10), client.read_to_end(&mut rest)).await??;
    assert!(rest.is_empty());
    assert!(
        timeout(Duration::from_millis(300), upstream.accept()).await.is_err(),
        "nothing is dialled for a prefix that is not a full record header"
    );
    Ok(())
}
