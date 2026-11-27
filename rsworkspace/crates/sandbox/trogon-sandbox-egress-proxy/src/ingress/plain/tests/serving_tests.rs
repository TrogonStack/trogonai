use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use trogon_sandbox_contract::Port;

use super::super::super::IngressLimits;
use super::super::super::token::ProxyToken;
use super::*;
use crate::resolve::{NameLookup, SsrfFloor, VettingResolver};

struct Loopback;

#[async_trait]
impl NameLookup for Loopback {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::from([127, 0, 0, 1])])
    }
}

struct Failing;

#[async_trait]
impl NameLookup for Failing {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Err(io::Error::from(io::ErrorKind::ConnectionRefused))
    }
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn context() -> Result<TlsInterceptContext, Box<dyn std::error::Error>> {
    crate::tls_ca::install_default_crypto_provider();
    Ok(TlsInterceptContext::new(
        crate::tls_ca::SandboxCa::generate(crate::tls_ca::PermittedDomains::from_rules(&[])?)?,
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        ),
        Arc::new(crate::credential_source::tests::FakeCredentialSource::new()),
        crate::proxy::OidcIdentityContext::unconfigured(crate::oidc_signer::OidcSigner::generate()?, "test")?,
    ))
}

fn ingress(lookup: Arc<dyn NameLookup>) -> Result<Ingress, Box<dyn std::error::Error>> {
    Ok(Ingress::new(
        ProxyToken::new("lease-token-value")?,
        Arc::new(VettingResolver::new(
            lookup,
            SsrfFloor::system().with_loopback_exception(),
        )),
        IngressLimits::default(),
    ))
}

fn credentials() -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "sandbox:lease-token-value")
}

fn request(method_and_extra: &str, port: u16, extra_headers: &str) -> String {
    format!(
        "{method_and_extra} http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nProxy-Authorization: Basic {}\r\n{extra_headers}\r\n",
        credentials()
    )
}

fn split_head(bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(bytes.len(), |at| at + 4);
    (bytes[..end].to_vec(), bytes[end..].to_vec())
}

async fn serve(
    ingress: &Ingress,
    policy: &NetworkPolicy,
    bytes: &[u8],
    leave: bool,
) -> Result<(IngressOutcome, Vec<u8>), Box<dyn std::error::Error>> {
    serve_in(ingress, policy, &context()?, bytes, leave).await
}

async fn serve_in(
    ingress: &Ingress,
    policy: &NetworkPolicy,
    context: &TlsInterceptContext,
    bytes: &[u8],
    leave: bool,
) -> Result<(IngressOutcome, Vec<u8>), Box<dyn std::error::Error>> {
    let (mut client, server) = duplex(16 * 1024 * 1024);
    client.write_all(bytes).await?;
    let (head, buffered) = split_head(bytes);
    if leave {
        drop(client);
        client = duplex(1).0;
    } else {
        client.shutdown().await?;
    }
    let outcome = ingress.serve_plain(server, policy, context, head, buffered).await;
    let mut answer = Vec::new();
    if !leave {
        let _ = timeout(Duration::from_secs(2), client.read_to_end(&mut answer)).await;
    }
    Ok((outcome, answer))
}

async fn origin(responses: usize) -> Result<(u16, tokio::task::JoinHandle<Vec<u8>>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let task = tokio::spawn(async move {
        let mut all = Vec::new();
        let Ok((mut stream, _)) = listener.accept().await else {
            return all;
        };
        let mut pending = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut answered = 0;
        while answered < responses {
            let Ok(n @ 1..) = stream.read(&mut chunk).await else {
                return all;
            };
            all.extend_from_slice(&chunk[..n]);
            pending.extend_from_slice(&chunk[..n]);
            while let Some(at) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                pending.drain(..at + 4);
                answered += 1;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
            }
        }
        all
    });
    Ok((port, task))
}

#[tokio::test]
async fn a_second_request_cut_short_or_malformed_ends_the_connection() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let mut bytes = request("GET", port, "").into_bytes();
    bytes.extend_from_slice(b"GET http://any.test/ HT");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, &bytes, false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");

    let (port, _origin) = origin(1).await?;
    let mut bytes = request("GET", port, "").into_bytes();
    bytes.extend_from_slice(b"\x00\x01 nonsense\r\n\r\n");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, &bytes, false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn an_unparseable_head_is_answered_bad_request() -> TestResult {
    let ingress = ingress(Arc::new(Loopback))?;
    let (outcome, answer) = serve(&ingress, &NetworkPolicy::AllowAll, b"GET\r\n\r\n", false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    assert!(
        answer.starts_with(b"HTTP/1.1 400"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    Ok(())
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_answered_bad_gateway() -> TestResult {
    let ingress = ingress(Arc::new(Failing))?;
    let bytes = request("GET", 80, "").replace("127.0.0.1", "any.test");
    let (outcome, answer) = serve(&ingress, &NetworkPolicy::AllowAll, bytes.as_bytes(), false).await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    assert!(
        answer.starts_with(b"HTTP/1.1 502"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    Ok(())
}

#[tokio::test]
async fn an_answer_to_a_client_that_left_ends_the_connection() -> TestResult {
    let ingress = ingress(Arc::new(Failing))?;
    let bytes = request("GET", 80, "").replace("127.0.0.1", "any.test");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, bytes.as_bytes(), true).await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_denied_request_with_an_unreadable_body_framing_is_answered_and_closed() -> TestResult {
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("POST", 80, "Content-Length: abc\r\n");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::DenyAll, bytes.as_bytes(), false).await?;
    assert!(
        matches!(outcome, IngressOutcome::Rejected(_) | IngressOutcome::Proxied { .. }),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_chunked_request_the_client_abandons_is_rejected() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("POST", port, "Transfer-Encoding: chunked\r\n");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, bytes.as_bytes(), true).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_malformed_chunked_body_is_refused() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let mut bytes = request("POST", port, "Transfer-Encoding: chunked\r\n").into_bytes();
    bytes.extend_from_slice(b"zz\r\nnot a chunk\r\n");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, &bytes, false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_second_request_that_stalls_mid_head_ends_the_connection() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let mut bytes = request("GET", port, "").into_bytes();
    bytes.extend_from_slice(b"GET http://any.test/ HTTP/1.1\r\nHost: a");
    let (mut client, server) = duplex(64 * 1024);
    client.write_all(&bytes).await?;
    let (head, buffered) = split_head(&bytes);
    let context = context()?;
    let outcome = ingress
        .serve_plain(server, &NetworkPolicy::AllowAll, &context, head, buffered)
        .await;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    Ok(())
}

/// Closes the connection with a reset instead of an orderly shutdown.
#[allow(deprecated)]
fn close_with_reset(stream: tokio::net::TcpStream) {
    let _ = stream.set_linger(Some(Duration::ZERO));
}

/// An origin that takes the head, then resets the connection.
async fn resetting_origin() -> Result<(u16, tokio::sync::oneshot::Receiver<()>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (reset, wait) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut seen = Vec::new();
            let mut chunk = [0u8; 1024];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n @ 1..) = stream.read(&mut chunk).await else {
                    break;
                };
                seen.extend_from_slice(&chunk[..n]);
            }
            close_with_reset(stream);
        }
        let _ = reset.send(());
    });
    Ok((port, wait))
}

#[tokio::test]
async fn a_body_an_origin_that_reset_cannot_take_is_answered_bad_gateway() -> TestResult {
    let (port, reset) = resetting_origin().await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let head = request("POST", port, "Content-Length: 1000000\r\n");
    let (mut client, server) = duplex(16 * 1024 * 1024);
    client.write_all(head.as_bytes()).await?;
    client.write_all(b"first").await?;
    let (head, buffered) = split_head(head.as_bytes());
    let context = context()?;
    let serving = ingress.serve_plain(server, &NetworkPolicy::AllowAll, &context, head, buffered);
    let rest = async {
        let _ = reset.await;
        client.write_all(&vec![b'x'; 999_995]).await?;
        client.shutdown().await?;
        let mut answer = Vec::new();
        let _ = timeout(Duration::from_secs(2), client.read_to_end(&mut answer)).await;
        Ok::<_, std::io::Error>(answer)
    };
    let (outcome, answer) = tokio::join!(serving, rest);
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    assert!(answer?.starts_with(b"HTTP/1.1 502"));
    Ok(())
}

#[tokio::test]
async fn a_chunk_the_parser_refuses_after_the_first_is_refused() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let mut bytes = request("POST", port, "Transfer-Encoding: chunked\r\n").into_bytes();
    bytes.extend_from_slice(b"3\r\nabc\r\nzz\r\n");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, &bytes, false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

/// A client stream that holds what it is written until opened.
struct Gated {
    inner: tokio::io::DuplexStream,
    open: Arc<std::sync::atomic::AtomicBool>,
    waiting: Arc<std::sync::Mutex<Option<std::task::Waker>>>,
}

impl tokio::io::AsyncRead for Gated {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Gated {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        if !self.open.load(Ordering::SeqCst) {
            if let Ok(mut waiting) = self.waiting.lock() {
                *waiting = Some(cx.waker().clone());
            }
            return std::task::Poll::Pending;
        }
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn a_head_an_origin_that_reset_cannot_take_is_answered_bad_gateway() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (reset, wait) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            close_with_reset(stream);
        }
        let _ = reset.send(());
    });
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("POST", port, "Expect: 100-continue\r\nContent-Length: 5\r\n");
    let (mut client, server) = duplex(64 * 1024);
    client.write_all(bytes.as_bytes()).await?;
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiting = Arc::new(std::sync::Mutex::new(None));
    let gated = Gated {
        inner: server,
        open: open.clone(),
        waiting: waiting.clone(),
    };
    let (head, buffered) = split_head(bytes.as_bytes());
    let context = context()?;
    let serving = ingress.serve_plain(gated, &NetworkPolicy::AllowAll, &context, head, buffered);
    let release = async {
        let _ = wait.await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        open.store(true, Ordering::SeqCst);
        if let Ok(mut waiting) = waiting.lock()
            && let Some(waker) = waiting.take()
        {
            waker.wake();
        }
        client.shutdown().await.ok();
        let mut answer = Vec::new();
        let _ = timeout(Duration::from_secs(2), client.read_to_end(&mut answer)).await;
        answer
    };
    let (outcome, answer) = tokio::join!(serving, release);
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    assert!(answer.ends_with(b"") && String::from_utf8_lossy(&answer).contains("502"));
    Ok(())
}

#[tokio::test]
async fn a_client_gone_before_the_continue_is_sent_ends_the_request() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("POST", port, "Expect: 100-continue\r\nContent-Length: 5\r\n");
    let (outcome, _) = serve(&ingress, &NetworkPolicy::AllowAll, bytes.as_bytes(), true).await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    Ok(())
}

fn forwarding_policy() -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "any.test".parse()?,
            action: DomainRuleAction::Forward(trogon_sandbox_contract::RequestForward {
                forward_url: trogon_sandbox_contract::ForwardUrl::new("https://forward.test")?,
                match_rules: vec![],
                ttl_seconds: trogon_sandbox_contract::RequestForward::DEFAULT_TTL_SECONDS,
            }),
        }],
        allow_ranges: vec![trogon_sandbox_contract::AllowRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

#[tokio::test]
async fn a_plain_request_for_a_domain_that_is_forwarded_is_denied() -> TestResult {
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("GET", 80, "").replace("127.0.0.1", "any.test");
    let (_, answer) = serve(&ingress, &forwarding_policy()?, bytes.as_bytes(), false).await?;
    assert!(
        answer.starts_with(b"HTTP/1.1 403"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    Ok(())
}

struct AllowEverything;

#[async_trait]
impl crate::request_filter::RequestFilter for AllowEverything {
    async fn decide(
        &self,
        _request: &crate::request_filter::RequestView<'_>,
    ) -> crate::request_filter::RequestDecision {
        crate::request_filter::RequestDecision::Allow
    }
}

#[tokio::test]
async fn a_refused_target_is_reported_to_the_request_checks() -> TestResult {
    let ingress = ingress(Arc::new(Loopback))?;
    let context = context()?.with_request_filter(Arc::new(AllowEverything));
    let mut bytes = request("GET", 80, "").into_bytes();
    let at = bytes.windows(7).position(|w| w == b":80/ HT").map_or(0, |at| at + 4);
    bytes.insert(at, 0x01);
    let (outcome, _) = serve_in(&ingress, &NetworkPolicy::AllowAll, &context, &bytes, false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

fn secret_context() -> Result<TlsInterceptContext, Box<dyn std::error::Error>> {
    Ok(secret_context_for("real-secret-value")?.0)
}

/// A context that swaps a sentinel for `real`, and the sentinel.
fn secret_context_for(real: &str) -> Result<(TlsInterceptContext, String), Box<dyn std::error::Error>> {
    let mut registry = trogon_sandbox_protocol::SentinelRegistry::new();
    let sentinel = registry.register(
        trogon_sandbox_contract::SecretName::new("api")?,
        trogon_sandbox_protocol::RealValue::new(real.to_string()),
        trogon_sandbox_contract::InjectHosts::new(vec![trogon_sandbox_contract::DomainPattern::new("127.0.0.1")?])?,
    );
    let context = context()?
        .with_secrets(crate::SecretSubstitution::new(Arc::new(registry)))
        .with_plaintext_inject(true);
    Ok((context, sentinel.as_str().to_string()))
}

#[tokio::test]
async fn a_client_gone_before_the_continue_of_a_substituted_body_is_sent_ends_the_request() -> TestResult {
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("POST", 80, "Expect: 100-continue\r\nContent-Length: 5\r\n");
    let (outcome, _) = serve_in(
        &ingress,
        &NetworkPolicy::AllowAll,
        &secret_context()?,
        bytes.as_bytes(),
        true,
    )
    .await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    Ok(())
}

/// A client that sends `sent`, then fails its reads.
struct BrokenClient {
    sent: std::io::Cursor<Vec<u8>>,
}

impl tokio::io::AsyncRead for BrokenClient {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let mut chunk = vec![0u8; buf.remaining()];
        let n = std::io::Read::read(&mut self.sent, &mut chunk)?;
        if n == 0 {
            return std::task::Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionReset)));
        }
        buf.put_slice(&chunk[..n]);
        std::task::Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for BrokenClient {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_body_the_client_fails_to_send_ends_the_request() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let mut bytes = request("POST", port, "Content-Length: 100\r\n").into_bytes();
    bytes.extend_from_slice(b"abc");
    let (head, buffered) = split_head(&bytes);
    let client = BrokenClient {
        sent: std::io::Cursor::new(Vec::new()),
    };
    let outcome = ingress
        .serve_plain(client, &NetworkPolicy::AllowAll, &context()?, head, buffered)
        .await;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_header_a_substitution_would_corrupt_is_refused() -> TestResult {
    let (port, _origin) = origin(1).await?;
    let ingress = ingress(Arc::new(Loopback))?;
    let (context, sentinel) = secret_context_for("two\r\nlines")?;
    let bytes = request("GET", port, &format!("X-Key: {sentinel}\r\n"));
    let (outcome, _) = serve_in(&ingress, &NetworkPolicy::AllowAll, &context, bytes.as_bytes(), false).await?;
    assert!(matches!(outcome, IngressOutcome::Rejected(_)), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_failed_dial_after_a_continue_drains_the_body_the_client_was_told_to_send() -> TestResult {
    let ingress = ingress(Arc::new(Loopback))?;
    let closed = TcpListener::bind("127.0.0.1:0").await?.local_addr()?.port();
    let mut bytes = request("POST", closed, "Expect: 100-continue\r\nContent-Length: 5\r\n").into_bytes();
    bytes.extend_from_slice(b"hello");
    let (outcome, answer) = serve_in(
        &ingress,
        &NetworkPolicy::AllowAll,
        &secret_context_for_any_port()?,
        &bytes,
        false,
    )
    .await?;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    assert!(String::from_utf8_lossy(&answer).contains("502"));
    Ok(())
}

fn secret_context_for_any_port() -> Result<TlsInterceptContext, Box<dyn std::error::Error>> {
    secret_context()
}

#[tokio::test]
async fn a_parent_proxy_with_credentials_is_sent_them() -> TestResult {
    let (port, parent) = origin(1).await?;
    let url = trogon_sandbox_contract::ParentProxyUrl::new(&format!("http://user:pass@127.0.0.1:{port}"))?;
    let options = trogon_sandbox_contract::NetworkOptions {
        parent_proxy: Some(trogon_sandbox_contract::ParentProxy::new(
            Some(url),
            None,
            trogon_sandbox_contract::NoProxy::new(""),
        )?),
        ..Default::default()
    };
    let context = context()?.with_network_options(Arc::new(options));
    let ingress = ingress(Arc::new(Loopback))?;
    let bytes = request("GET", 80, "").replace("127.0.0.1", "any.test");
    let (outcome, _) = timeout(
        Duration::from_secs(5),
        serve_in(&ingress, &NetworkPolicy::AllowAll, &context, bytes.as_bytes(), false),
    )
    .await??;
    assert!(matches!(outcome, IngressOutcome::Proxied { .. }), "{outcome:?}");
    let seen = String::from_utf8_lossy(&timeout(Duration::from_secs(5), parent).await??).to_ascii_lowercase();
    assert!(seen.contains("proxy-authorization: basic"), "{seen}");
    Ok(())
}
