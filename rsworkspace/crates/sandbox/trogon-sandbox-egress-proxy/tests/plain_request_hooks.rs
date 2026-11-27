use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::{AllowRange, LeaseId, NetworkPolicy};
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer, ProxyServerBuilder};
use trogon_sandbox_egress_proxy::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{ViolationRateLimit, ViolationReporter};
use trogon_sandbox_egress_proxy::{
    CallbackRequestFilter, CommandId, DenialReasons, HeaderEdits, HeaderMutator, RequestAllow, RequestDecision,
    RequestDenial, RequestFilter, RequestInfo, RequestView, StoreViolationSink, ViolationStore,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const TOKEN: &str = "lease-token-value";

type Captured = Arc<Mutex<Vec<Vec<u8>>>>;

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn ca() -> TestResult<SandboxCa> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::from_rules(&[])?)?)
}

/// The requests the upstream captured so far.
fn captures(captured: &Captured) -> TestResult<Vec<Vec<u8>>> {
    Ok(captured.lock().map_err(|_| "capture lock poisoned")?.clone())
}

/// The first request the upstream captured.
fn first_capture(captured: &Captured) -> TestResult<Vec<u8>> {
    captures(captured)?
        .into_iter()
        .next()
        .ok_or_else(|| "the upstream saw no request".into())
}

/// A copy of what a shared record holds.
fn recorded<T: Clone>(record: &Mutex<T>) -> TestResult<T> {
    Ok(record.lock().map_err(|_| "record lock poisoned")?.clone())
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

fn body_gate(expected: &'static [u8]) -> Arc<CallbackRequestFilter> {
    Arc::new(CallbackRequestFilter::new(Arc::new(
        move |request: RequestInfo| async move {
            let Ok(shown) = request.body.read(1024).await else {
                return RequestDecision::from(false);
            };
            RequestDecision::from(shown.bytes == expected && !shown.cut)
        },
    )))
}

#[tokio::test]
async fn a_callback_reads_a_plain_body_and_the_upstream_still_gets_it_whole() -> TestResult {
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.request_filter(body_gate(b"hello"))).await?;

    let reply = exchange(proxy, port, "Content-Length: 5\r\n", b"hello", false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    assert!(first_capture(&captured)?.ends_with(b"\r\n\r\nhello"));
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_callback_denying_on_the_plain_body_keeps_it_from_the_upstream() -> TestResult {
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.request_filter(body_gate(b"hello"))).await?;

    let reply = exchange(proxy, port, "Content-Length: 5\r\n", b"other", false).await?;
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
    assert!(captures(&captured)?.is_empty());
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_chunked_plain_body_is_shown_decoded_and_forwarded_chunked() -> TestResult {
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.request_filter(body_gate(b"hello"))).await?;

    let reply = exchange(
        proxy,
        port,
        "Transfer-Encoding: chunked\r\n",
        b"5\r\nhello\r\n0\r\n\r\n",
        false,
    )
    .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    assert!(first_capture(&captured)?.ends_with(b"5\r\nhello\r\n0\r\n\r\n"));
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn expect_continue_is_answered_once_before_the_callback_reads_the_body() -> TestResult {
    let (port, captured) = upstream().await?;
    let (server, proxy) = start(|b| b.request_filter(body_gate(b"hello"))).await?;

    let reply = exchange(
        proxy,
        port,
        "Content-Length: 5\r\nExpect: 100-continue\r\n",
        b"hello",
        true,
    )
    .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    assert!(first_capture(&captured)?.ends_with(b"\r\n\r\nhello"));
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn the_plaintext_mutator_edits_the_forwarded_headers_for_the_destination() -> TestResult {
    let (port, captured) = upstream().await?;
    let destinations: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = destinations.clone();
    let mutator: Arc<dyn HeaderMutator> = Arc::new(move |headers: &mut Vec<(String, String)>, destination: &Host| {
        if let Ok(mut destinations) = seen.lock() {
            destinations.push(destination.to_string());
        }
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case("x-drop"));
        headers.push(("X-Injected".to_string(), "yes".to_string()));
    });
    let (server, proxy) = start(|b| b.plaintext_header_mutator(mutator)).await?;

    let reply = exchange(proxy, port, "X-Drop: 1\r\nContent-Length: 2\r\n", b"hi", false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let forwarded = String::from_utf8(first_capture(&captured)?)?;
    assert!(forwarded.contains("X-Injected: yes\r\n"), "{forwarded}");
    assert!(!forwarded.to_ascii_lowercase().contains("x-drop"), "{forwarded}");
    assert_eq!(recorded(&destinations)?, vec!["127.0.0.1".to_string()]);
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_mutator_that_changes_the_framing_is_refused() -> TestResult {
    let (port, captured) = upstream().await?;
    let mutator: Arc<dyn HeaderMutator> = Arc::new(|headers: &mut Vec<(String, String)>, _: &Host| {
        for (name, value) in headers.iter_mut() {
            if name.eq_ignore_ascii_case("content-length") {
                *value = "99".to_string();
            }
        }
    });
    let (server, proxy) = start(|b| b.plaintext_header_mutator(mutator)).await?;

    let reply = exchange(proxy, port, "Content-Length: 2\r\n", b"hi", false).await?;
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
    assert!(captures(&captured)?.is_empty());
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn the_lease_token_reaches_no_callback_mutator_or_upstream_on_the_plain_path() -> TestResult {
    let (port, captured) = upstream().await?;
    let shown: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let mutated: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let callback = {
        let shown = shown.clone();
        Arc::new(CallbackRequestFilter::new(Arc::new(move |request: RequestInfo| {
            let shown = shown.clone();
            async move {
                if let Ok(mut shown) = shown.lock() {
                    shown.extend(request.headers);
                }
                RequestDecision::Allow
            }
        })))
    };
    let mutator: Arc<dyn HeaderMutator> = {
        let mutated = mutated.clone();
        Arc::new(move |headers: &mut Vec<(String, String)>, _: &Host| {
            if let Ok(mut mutated) = mutated.lock() {
                mutated.extend(headers.iter().cloned());
            }
        })
    };
    let (server, proxy) = start(|b| b.request_filter(callback).plaintext_header_mutator(mutator)).await?;

    let reply = exchange(
        proxy,
        port,
        "Proxy-Connection: keep-alive\r\nContent-Length: 2\r\n",
        b"hi",
        false,
    )
    .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    for headers in [&shown, &mutated] {
        let headers = recorded(headers)?;
        assert!(!headers.is_empty());
        assert!(headers.iter().all(|(name, value)| {
            !name.eq_ignore_ascii_case("proxy-authorization")
                && !name.eq_ignore_ascii_case("proxy-connection")
                && !value.contains(TOKEN)
        }));
    }
    let forwarded = String::from_utf8(first_capture(&captured)?)?.to_ascii_lowercase();
    assert!(!forwarded.contains("proxy-"), "{forwarded}");
    assert!(!forwarded.contains(&STANDARD.encode(format!("sandbox:{TOKEN}")).to_ascii_lowercase()));
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_denied_plain_request_reaches_the_store_without_its_userinfo_or_query() -> TestResult {
    let store = ViolationStore::new();
    let sink = Arc::new(StoreViolationSink::new(
        store.clone(),
        CommandId::new("c1"),
        DenialReasons::new(),
    ));
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink,
        ViolationRateLimit::default(),
    ));
    let filter = Arc::new(CallbackRequestFilter::new(Arc::new(|_: RequestInfo| async {
        RequestDecision::DenyWith(RequestDenial::new().reason("policy says no"))
    })));
    let (server, proxy) = start(|b| b.request_filter(filter).violations(reporter)).await?;

    let send = |target: &'static str| async move {
        let mut client = TcpStream::connect(proxy).await?;
        let head = format!(
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:9\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
            basic()
        );
        client.write_all(head.as_bytes()).await?;
        let mut out = Vec::new();
        let _ = client.read_to_end(&mut out).await;
        Ok::<String, std::io::Error>(String::from_utf8_lossy(&out).into_owned())
    };
    let reply = send("http://alice:PASSW0RD@127.0.0.1:9/x?access_token=SECRET123").await?;
    assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
    let reply = send("http://127.0.0.1:9/x?access_token=SECRET123").await?;
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");

    let mut lines = Vec::new();
    for _ in 0..100 {
        lines = store
            .violations(None)
            .iter()
            .map(|violation| violation.line.as_str().to_string())
            .collect();
        if !lines.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let joined = lines.join("\n");
    assert!(
        lines.contains(&"deny http-request GET http://127.0.0.1:9/x?\u{2026} (policy says no)".to_string()),
        "{lines:?}"
    );
    for secret in ["SECRET123", "PASSW0RD", "alice"] {
        assert!(!joined.contains(secret), "{joined}");
    }
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

struct Allowing(RequestAllow);

#[async_trait::async_trait]
impl RequestFilter for Allowing {
    async fn decide(&self, _: &RequestView<'_>) -> RequestDecision {
        RequestDecision::AllowWith(self.0.clone())
    }
}

fn edits<E: std::error::Error + 'static>(
    build: impl FnOnce(HeaderEdits) -> Result<HeaderEdits, E>,
) -> TestResult<Arc<Allowing>> {
    Ok(Arc::new(Allowing(
        RequestAllow::new().edits(build(HeaderEdits::new())?),
    )))
}

#[tokio::test]
async fn a_header_set_on_a_plain_request_is_refused_unless_the_opt_in_is_on() -> TestResult {
    let (port, captured) = upstream().await?;
    let filter = edits(|e| e.set("x-added", "1"))?;
    let (server, proxy) = start(|b| b.request_filter(filter)).await?;

    let reply = exchange(proxy, port, "", b"", false).await?;
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
    assert!(
        reply
            .to_ascii_lowercase()
            .contains("x-proxy-error: blocked-by-egress-proxy"),
        "{reply}"
    );
    assert!(captures(&captured)?.is_empty());
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_header_set_on_a_plain_request_is_applied_when_the_opt_in_is_on() -> TestResult {
    let (port, captured) = upstream().await?;
    let filter = edits(|e| e.set("x-added", "1"))?;
    let (server, proxy) = start(|b| b.request_filter(filter).plaintext_header_set(true)).await?;

    let reply = exchange(proxy, port, "", b"", false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let forwarded = String::from_utf8(first_capture(&captured)?)?;
    assert!(forwarded.contains("x-added: 1\r\n"), "{forwarded}");
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn removals_alone_are_allowed_on_a_plain_request_without_the_opt_in() -> TestResult {
    let (port, captured) = upstream().await?;
    let filter = edits(|e| e.remove("x-secret"))?;
    let (server, proxy) = start(|b| b.request_filter(filter)).await?;

    let reply = exchange(proxy, port, "X-Secret: s\r\nX-Keep: k\r\n", b"", false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let forwarded = String::from_utf8(first_capture(&captured)?)?.to_ascii_lowercase();
    assert!(!forwarded.contains("x-secret"), "{forwarded}");
    assert!(forwarded.contains("x-keep: k"), "{forwarded}");
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn an_empty_allow_is_allowed_on_a_plain_request_without_the_opt_in() -> TestResult {
    let (port, captured) = upstream().await?;
    let filter = edits(Ok::<_, std::convert::Infallible>)?;
    let (server, proxy) = start(|b| b.request_filter(filter)).await?;

    let reply = exchange(proxy, port, "", b"", false).await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    assert_eq!(captures(&captured)?.len(), 1);
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_removed_header_is_stripped_in_every_separator_spelling() -> TestResult {
    let (port, captured) = upstream().await?;
    let filter = edits(|e| e.remove("x-api-key")?.remove("cookie")?.remove("authorization"))?;
    let (server, proxy) = start(|b| b.request_filter(filter)).await?;

    let reply = exchange(
        proxy,
        port,
        "Authorization: Bearer sandbox\r\nX_Api_Key: k1\r\nx.api.key: k2\r\nX-Api-Key: k3\r\nCookie: a=b\r\nX-Keep: k\r\n",
        b"",
        false,
    )
    .await?;
    assert!(reply.starts_with("HTTP/1.1 200 "), "{reply}");
    let forwarded = String::from_utf8(first_capture(&captured)?)?.to_ascii_lowercase();
    for gone in [
        "x_api_key",
        "x.api.key",
        "x-api-key",
        "cookie",
        "authorization",
        "sandbox",
    ] {
        assert!(!forwarded.contains(gone), "{gone}: {forwarded}");
    }
    assert!(forwarded.contains("x-keep: k"), "{forwarded}");
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn a_non_utf8_plain_target_is_recorded_with_the_replacement_character_percent_encoded() -> TestResult {
    let store = ViolationStore::new();
    let sink = Arc::new(StoreViolationSink::new(
        store.clone(),
        CommandId::new("c1"),
        DenialReasons::new(),
    ));
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink,
        ViolationRateLimit::default(),
    ));
    let filter = edits(Ok::<_, std::convert::Infallible>)?;
    let (server, proxy) = start(|b| b.request_filter(filter).violations(reporter)).await?;

    for tail in ["/caf\u{e9}", "/caf\u{e9}#t=SECRET"] {
        let mut client = TcpStream::connect(proxy).await?;
        let mut raw = b"GET http://127.0.0.1:9/caf".to_vec();
        raw.push(0xe9);
        if let Some((_, fragment)) = tail.split_once('#') {
            raw.extend_from_slice(format!("#{fragment}").as_bytes());
        }
        raw.extend_from_slice(
            format!(
                " HTTP/1.1\r\nHost: 127.0.0.1:9\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
                basic()
            )
            .as_bytes(),
        );
        client.write_all(&raw).await?;
        let mut out = Vec::new();
        let _ = client.read_to_end(&mut out).await;
        let reply = String::from_utf8_lossy(&out).into_owned();
        assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
    }

    let mut lines = Vec::new();
    for _ in 0..100 {
        lines = store
            .violations(None)
            .iter()
            .map(|violation| violation.line.as_str().to_string())
            .collect();
        if lines.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert!(
            line.starts_with("deny http-request GET http://127.0.0.1:9/caf%EF%BF%BD (malformed request-target: "),
            "{line}"
        );
        assert!(line.contains("bytes that are not UTF-8"), "{line}");
        assert!(!line.contains("SECRET"), "{line}");
    }
    server.shutdown(std::time::Duration::from_millis(200)).await?;
    Ok(())
}
