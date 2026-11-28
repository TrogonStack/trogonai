use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::net::{TcpListener, TcpStream};
use trogon_sandbox_contract::{
    AllowRange, EgressViolation, EgressViolationReason, HttpMethod, LeaseId, NetworkPolicy, PathPrefix, RequestRule,
    RequestRuleAction,
};
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer};
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

#[derive(Default)]
struct InMemorySink(Mutex<Vec<Vec<u8>>>);

#[async_trait]
impl ViolationSink for InMemorySink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        locked(&self.0).push(payload);
        Ok(())
    }
}

impl InMemorySink {
    async fn wait_for(&self, count: usize) -> TestResult<Vec<EgressViolation>> {
        for _ in 0..100 {
            {
                let events = locked(&self.0);
                if events.len() >= count {
                    let parsed: Result<Vec<EgressViolation>, _> =
                        events.iter().map(|payload| serde_json::from_slice(payload)).collect();
                    return Ok(parsed?);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err(format!("expected {count} violations").into())
    }
}

fn policy(allow_loopback: bool) -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: if allow_loopback {
            vec![AllowRange::new("127.0.0.0/8")?]
        } else {
            vec![]
        },
        deny_ranges: vec![],
        denied_domains: vec!["blocked.test".parse()?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn ca() -> TestResult<SandboxCa> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::from_rules(&[])?)?)
}

fn basic() -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{TOKEN}")))
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

async fn read_head<S: AsyncReadExt + Unpin>(stream: &mut S) -> TestResult<String> {
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

async fn connect_via<S: AsyncReadExt + AsyncWriteExt + Unpin>(stream: &mut S, target: &str) -> TestResult<String> {
    stream
        .write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
                basic()
            )
            .as_bytes(),
        )
        .await?;
    read_head(stream).await
}

async fn echo_roundtrip<S: AsyncReadExt + AsyncWriteExt + Unpin>(stream: &mut S) -> TestResult {
    stream.write_all(b"ping").await?;
    let mut got = [0u8; 4];
    stream.read_exact(&mut got).await?;
    assert_eq!(&got, b"ping");
    Ok(())
}

fn reporter(sink: &Arc<InMemorySink>) -> Arc<ViolationReporter> {
    Arc::new(ViolationReporter::new(
        LeaseId::new("L1".to_string()),
        sink.clone(),
        ViolationRateLimit::default(),
    ))
}

fn tcp_addr(server: &ProxyServer) -> TestResult<SocketAddr> {
    match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => Ok(*addr),
        other => Err(format!("{other:?}").into()),
    }
}

#[tokio::test]
async fn embedded_server_enforces_policy_swaps_it_and_shuts_down() -> TestResult {
    let upstream = echo_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy(false)?, ca()?)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .violations(reporter(&sink))
        .start()?;
    let proxy = tcp_addr(&server)?;
    assert!(server.ca_certificate_pem().starts_with("-----BEGIN CERTIFICATE-----"));

    let local = format!("127.0.0.1:{upstream}");

    let mut denied = TcpStream::connect(proxy).await?;
    let head = connect_via(&mut denied, "blocked.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    let events = sink.wait_for(1).await?;
    assert_eq!(events[0].host.as_deref(), Some("blocked.test"));
    assert_eq!(events[0].port, 443);

    let mut before_swap = TcpStream::connect(proxy).await?;
    let head = connect_via(&mut before_swap, &local).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");

    server.replace_policy(policy(true)?).await?;

    let mut allowed = TcpStream::connect(proxy).await?;
    let head = connect_via(&mut allowed, &local).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut allowed).await?;

    server.shutdown(Duration::from_millis(200)).await?;

    assert!(
        TcpStream::connect(proxy).await.is_err(),
        "listener must be closed after shutdown"
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_waits_for_a_tunnel_in_flight_within_the_grace_period() -> TestResult {
    let upstream = echo_upstream().await?;
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy(true)?, ca()?)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .start()?;
    let proxy = tcp_addr(&server)?;

    let mut tunnel = TcpStream::connect(proxy).await?;
    let head = connect_via(&mut tunnel, &format!("127.0.0.1:{upstream}")).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");

    let shutdown = tokio::spawn(server.shutdown(Duration::from_secs(5)));
    echo_roundtrip(&mut tunnel).await?;
    assert!(!shutdown.is_finished());
    drop(tunnel);
    tokio::time::timeout(Duration::from_secs(5), shutdown).await???;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn embedded_server_serves_a_unix_socket() -> TestResult {
    let upstream = echo_upstream().await?;
    let dir = std::env::temp_dir().join(format!("sandbox-egress-server-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("proxy.sock");
    let listener = Listener::bind(&ListenAddress::Unix(path.clone())).await?;
    let server = ProxyServer::builder(policy(true)?, ca()?)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .start()?;
    assert_eq!(server.explicit_addr(), Some(&BoundAddress::Unix(path.clone())));

    let mut stream = UnixStream::connect(&path).await?;
    let head = connect_via(&mut stream, &format!("127.0.0.1:{upstream}")).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut stream).await?;
    drop(stream);

    server.shutdown(Duration::from_millis(200)).await?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn starting_without_a_listener_is_an_error() -> TestResult {
    assert!(ProxyServer::builder(policy(false)?, ca()?).start().is_err());
    Ok(())
}

#[tokio::test]
async fn embedded_server_applies_request_rules_and_reports_denials() -> TestResult {
    let upstream = echo_upstream().await?;
    let sink = Arc::new(InMemorySink::default());
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let mut policy = policy(true)?;
    if let NetworkPolicy::UserDefined { request_rules, .. } = &mut policy {
        request_rules.push(RequestRule {
            action: RequestRuleAction::Deny,
            domains: Vec::new(),
            methods: vec![HttpMethod::new("DELETE")?],
            path_prefix: Some(PathPrefix::new("/admin")?),
            headers: Vec::new(),
        });
    }
    let server = ProxyServer::builder(policy, ca()?)
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .violations(reporter(&sink))
        .start()?;
    let proxy = tcp_addr(&server)?;

    let mut client = TcpStream::connect(proxy).await?;
    client
        .write_all(
            format!(
                "DELETE http://127.0.0.1:{upstream}/admin//users HTTP/1.1\r\nHost: 127.0.0.1:{upstream}\r\nProxy-Authorization: {}\r\nContent-Length: 0\r\n\r\n",
                basic()
            )
            .as_bytes(),
        ).await?;
    let head = read_head(&mut client).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");

    let events = sink.wait_for(1).await?;
    assert_eq!(events[0].reason, EgressViolationReason::RequestDenied);

    server.shutdown(Duration::from_millis(200)).await?;
    Ok(())
}
