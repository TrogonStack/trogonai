//! Raw bytes through an explicit-proxy CONNECT are refused unless the policy opts in.
//!
//! Ported from the exfiltration matrix (cases 9 and 9b): a sandboxed process with `HTTPS_PROXY` opens a CONNECT to
//! an allowed host and port and then writes bytes that are neither TLS nor HTTP, carrying a secret sentinel. With a
//! policy that says nothing about opaque tunnels, those bytes must never reach the upstream.
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
const MARKER: &str = "NOTHTTP-EXFIL";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type Received = Arc<Mutex<Vec<u8>>>;

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

/// An upstream that records every byte it is sent.
async fn recording_upstream() -> TestResult<(u16, Received)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let received: Received = Arc::default();
    let sink = received.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut chunk = [0u8; 4096];
                while let Ok(n) = stream.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    locked(&sink).extend_from_slice(&chunk[..n]);
                }
            });
        }
    });
    Ok((port, received))
}

struct Harness {
    server: ProxyServer,
    proxy: SocketAddr,
    upstream: u16,
    received: Received,
    sentinel: String,
}

/// Every destination the policies below name; only `bound.test` carries a secret binding, which makes the proxy
/// terminate it.
const BOUND: &str = "bound.test";
const PLAIN: &str = "plain.test";

fn rules(upstream: u16) -> TestResult<Vec<DomainRule>> {
    [BOUND, PLAIN]
        .into_iter()
        .map(|host| {
            Ok(DomainRule {
                domain: format!("{host}:{upstream}").parse()?,
                action: DomainRuleAction::PassThrough,
            })
        })
        .collect()
}

fn policy(upstream: u16, opaque_tunnels: Option<OpaqueTunnelPolicy>) -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: rules(upstream)?,
        // Loopback is below the SSRF floor; carve out exactly the one upstream address and port.
        allow_ranges: vec![AllowRange::new(format!("127.0.0.1:{upstream}"))?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: opaque_tunnels.unwrap_or_default(),
        request_rules: vec![],
    })
}

/// The same policy as `policy(upstream, None)`, read from JSON that never mentions `opaque_tunnels`.
fn policy_from_wire(upstream: u16) -> TestResult<NetworkPolicy> {
    let rules: Vec<_> = [BOUND, PLAIN]
        .into_iter()
        .map(|host| serde_json::json!({"domain": format!("{host}:{upstream}"), "action": "pass_through"}))
        .collect();
    Ok(serde_json::from_value(serde_json::json!({"user_defined": {
        "rules": rules,
        "allow_ranges": [format!("127.0.0.1:{upstream}")],
        "deny_ranges": [],
    }}))?)
}

async fn start(policy_for: impl FnOnce(u16) -> TestResult<NetworkPolicy>) -> TestResult<Harness> {
    install_default_crypto_provider();
    let (upstream, received) = recording_upstream().await?;
    let policy = policy_for(upstream)?;
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(
        SecretName::new("api")?,
        RealValue::new(REAL.to_string()),
        InjectHosts::new(vec![DomainPattern::new(BOUND)?])?,
    );
    let sentinel = sentinel.as_str().to_string();
    let ca = SandboxCa::generate(PermittedDomains::from_rules(&rules(upstream)?)?)?;
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy, ca)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .resolver(Arc::new(VettingResolver::new(Arc::new(Loopback), SsrfFloor::system())))
        .secrets(SecretSubstitution::new(Arc::new(registry)))
        .start()?;
    let proxy = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("{other:?}").into()),
    };
    Ok(Harness {
        server,
        proxy,
        upstream,
        received,
        sentinel,
    })
}

/// Opens a CONNECT to `host`, writes the opaque payload straight after the head whatever the answer, and returns
/// the status line the proxy sent.
async fn connect_and_send_opaque_bytes(h: &Harness, host: &str) -> TestResult<String> {
    let mut client = TcpStream::connect(h.proxy).await?;
    let auth = STANDARD.encode(format!("sandbox:{TOKEN}"));
    let target = format!("{host}:{}", h.upstream);
    client
        .write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n{MARKER} {}\r\n\r\n",
                h.sentinel
            )
            .as_bytes(),
        )
        .await?;
    let mut reply = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut reply)).await;
    Ok(String::from_utf8_lossy(&reply)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string())
}

async fn upstream_received(h: &Harness) -> String {
    tokio::time::sleep(Duration::from_millis(300)).await;
    String::from_utf8_lossy(&locked(&h.received)).into_owned()
}

/// Exfil case 9: the host has a secret binding, so the proxy answers `200` and terminates the tunnel, and the
/// first bytes are not TLS. They must not be spliced through to the upstream.
#[tokio::test]
async fn raw_bytes_into_a_terminated_tunnel_never_reach_the_upstream_by_default() -> TestResult {
    let h = start(|upstream| policy(upstream, None)).await?;
    let status = connect_and_send_opaque_bytes(&h, BOUND).await?;
    let received = upstream_received(&h).await;
    assert!(
        !received.contains(MARKER),
        "opaque bytes reached the upstream after {status}: {received:?}"
    );
    assert!(!received.contains(REAL));
    h.server.shutdown(Duration::from_millis(200)).await?;
    Ok(())
}

/// A pass-through host the proxy would relay unread is refused before the tunnel opens.
#[tokio::test]
async fn an_unterminated_connect_is_refused_by_default() -> TestResult {
    let h = start(|upstream| policy(upstream, None)).await?;
    let status = connect_and_send_opaque_bytes(&h, PLAIN).await?;
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    assert!(
        !upstream_received(&h).await.contains(MARKER),
        "opaque bytes reached the upstream"
    );
    h.server.shutdown(Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_policy_parsed_without_the_field_refuses_opaque_tunnels() -> TestResult {
    let h = start(policy_from_wire).await?;
    let status = connect_and_send_opaque_bytes(&h, PLAIN).await?;
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    let status = connect_and_send_opaque_bytes(&h, BOUND).await?;
    let received = upstream_received(&h).await;
    assert!(
        !received.contains(MARKER),
        "opaque bytes reached the upstream after {status}: {received:?}"
    );
    h.server.shutdown(Duration::from_millis(200)).await?;
    Ok(())
}

/// Exfil case 9b, inverted: the opt-in still carries a non-TLS protocol, and the sentinel is not swapped for the
/// real value in bytes the proxy never parsed.
#[tokio::test]
async fn a_policy_that_opts_in_still_tunnels_raw_bytes() -> TestResult {
    let h = start(|upstream| policy(upstream, Some(OpaqueTunnelPolicy::Allow))).await?;
    let status = connect_and_send_opaque_bytes(&h, BOUND).await?;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    let received = upstream_received(&h).await;
    assert!(received.contains(MARKER), "the opted-in tunnel carried nothing");
    assert!(!received.contains(REAL));
    h.server.shutdown(Duration::from_millis(200)).await?;
    Ok(())
}
