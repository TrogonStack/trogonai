use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_sandbox_contract::{DomainPattern, InjectHosts, SecretName};
use trogon_sandbox_contract::{IpRange, NetworkPolicy};
use trogon_sandbox_egress_proxy::SecretSubstitution;
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer, ProxyServerBuilder};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

const TOKEN: &str = "lease-token-value";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type Captured = Arc<Mutex<Vec<Vec<u8>>>>;

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![IpRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    })
}

fn ca() -> TestResult<SandboxCa> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::from_rules(&[])?)?)
}

/// The request bytes the upstream saw first.
fn first_capture(captured: &Captured) -> TestResult<Vec<u8>> {
    let guard = captured.lock().map_err(|_| "capture lock poisoned")?;
    Ok(guard.first().ok_or("the upstream saw no request")?.clone())
}

fn basic() -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{TOKEN}")))
}

fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|at| at + 4)
}

async fn upstream() -> TestResult<(u16, Captured)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let captured: Captured = Arc::default();
    let sink = captured.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let n = stream.read(&mut chunk).await.map_or(0, |read| read);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(head) = header_end(&buf) else {
                        continue;
                    };
                    let text = String::from_utf8_lossy(&buf[..head]).to_ascii_lowercase();
                    let done = if text.contains("transfer-encoding: chunked") {
                        buf.ends_with(b"0\r\n\r\n")
                    } else {
                        let length = text
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map_or(0, |value| value.trim().parse::<usize>().map_or(0, |n| n));
                        buf.len() >= head + length
                    };
                    if done {
                        break;
                    }
                }
                if let Ok(mut seen) = sink.lock() {
                    seen.push(buf);
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Ok((port, captured))
}

async fn start(
    configure: impl FnOnce(ProxyServerBuilder) -> ProxyServerBuilder,
) -> TestResult<(ProxyServer, SocketAddr)> {
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server =
        configure(ProxyServer::builder(policy()?, ca()?).explicit(listener, ProxyToken::new(TOKEN)?)).start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("{other:?}").into()),
    };
    Ok((server, addr))
}

async fn exchange(
    proxy: SocketAddr,
    upstream: u16,
    request_tail: &str,
    body: &[u8],
    send_after_continue: bool,
) -> TestResult<String> {
    let mut client = TcpStream::connect(proxy).await?;
    let head = format!(
        "POST http://127.0.0.1:{upstream}/submit HTTP/1.1\r\nHost: 127.0.0.1:{upstream}\r\nProxy-Authorization: {}\r\n{request_tail}\r\n",
        basic()
    );
    client.write_all(head.as_bytes()).await?;
    if send_after_continue {
        let mut interim = [0u8; 25];
        client.read_exact(&mut interim).await?;
        assert!(String::from_utf8_lossy(&interim).starts_with("HTTP/1.1 100 Continue"));
    }
    client.write_all(body).await?;
    let mut out = Vec::new();
    let _ = client.read_to_end(&mut out).await;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn registry(hosts: &str) -> TestResult<(Arc<SentinelRegistry>, String)> {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(
        SecretName::new("api")?,
        RealValue::new("real-secret-value".to_string()),
        InjectHosts::new(vec![DomainPattern::new(hosts)?])?,
    );
    Ok((Arc::new(registry), sentinel.as_str().to_string()))
}

#[tokio::test]
async fn plaintext_inject_swaps_sentinels_in_headers_and_body_at_the_bound_host() -> TestResult {
    let (registry, sentinel) = registry("127.0.0.1")?;
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.secrets(SecretSubstitution::new(registry)).plaintext_inject(true)).await?;

    let body = format!("token={sentinel}");
    let tail = format!("Authorization: Bearer {sentinel}\r\nContent-Length: {}\r\n", body.len());
    let reply = exchange(proxy, port, &tail, body.as_bytes(), false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let seen = String::from_utf8_lossy(&first_capture(&captured)?).into_owned();
    assert!(seen.contains("authorization: Bearer real-secret-value"), "{seen}");
    assert!(!seen.contains("content-length"), "{seen}");
    assert!(seen.contains("transfer-encoding: chunked"), "{seen}");
    assert!(seen.contains("token=real-secret-value"), "{seen}");
    assert!(!seen.contains(&sentinel), "{seen}");
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_plain_body_over_the_default_body_limit_is_streamed_and_substituted() -> TestResult {
    let (registry, sentinel) = registry("127.0.0.1")?;
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.secrets(SecretSubstitution::new(registry)).plaintext_inject(true)).await?;

    let mut body = vec![b'x'; 9 * 1024 * 1024];
    body.extend_from_slice(sentinel.as_bytes());
    let tail = format!("Content-Length: {}\r\n", body.len());
    let reply = exchange(proxy, port, &tail, &body, false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let seen = first_capture(&captured)?;
    assert!(seen.ends_with(b"real-secret-value\r\n0\r\n\r\n"));
    assert!(seen.len() > 9 * 1024 * 1024);
    assert!(!String::from_utf8_lossy(&seen).contains(&sentinel));
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn plain_requests_keep_their_sentinels_unless_plaintext_inject_is_on() -> TestResult {
    let (registry, sentinel) = registry("127.0.0.1")?;
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.secrets(SecretSubstitution::new(registry))).await?;

    let body = format!("token={sentinel}");
    let tail = format!("Content-Length: {}\r\n", body.len());
    let reply = exchange(proxy, port, &tail, body.as_bytes(), false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let seen = String::from_utf8_lossy(&first_capture(&captured)?).into_owned();
    assert!(seen.ends_with(&body), "{seen}");
    assert!(!seen.contains("real-secret-value"), "{seen}");
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn plaintext_inject_leaves_sentinels_alone_off_their_host() -> TestResult {
    let (registry, sentinel) = registry("api.example.com")?;
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.secrets(SecretSubstitution::new(registry)).plaintext_inject(true)).await?;

    let body = format!("token={sentinel}");
    let tail = format!("Content-Length: {}\r\n", body.len());
    exchange(proxy, port, &tail, body.as_bytes(), false).await?;
    let seen = String::from_utf8_lossy(&first_capture(&captured)?).into_owned();
    assert!(seen.contains(&sentinel), "{seen}");
    assert!(!seen.contains("real-secret-value"), "{seen}");
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}
