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
use tokio::sync::mpsc;
use trogon_sandbox_contract::Port;
use trogon_sandbox_contract::{
    AllowRange, DomainRule, DomainRuleAction, IpRange, LeaseId, NetworkPolicy, SandboxId, TenantId,
};
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;
use trogon_sandbox_egress_proxy::ingress::{Ingress, IngressLimits, IngressOutcome, ProxyToken};
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
    calls: AtomicUsize,
    answers: Vec<IpAddr>,
}

#[async_trait]
impl NameLookup for StubLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.answers.clone())
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
    outcomes: mpsc::UnboundedReceiver<IngressOutcome>,
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
    let network = Arc::new(network);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let resolver = Arc::new(StubLookup {
        calls: AtomicUsize::new(0),
        answers,
    });
    let sink = Arc::new(CollectingSink::default());
    let reporter = Arc::new(ViolationReporter::new(
        LeaseId::new("L1"),
        sink.clone(),
        ViolationRateLimit::default(),
    ));
    let ingress = Arc::new(Ingress::new(
        ProxyToken::new(TOKEN)?,
        Arc::new(VettingResolver::new(resolver.clone(), SsrfFloor::system())),
        limits,
    ));
    let tls = Arc::new(tls_context()?);
    let (tx, outcomes) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
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
    Ok(Harness { addr, outcomes })
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

type Seen = Arc<Mutex<Vec<Vec<u8>>>>;

fn seen_requests(seen: &Seen) -> TestResult<Vec<Vec<u8>>> {
    Ok(seen.lock().map_err(|_| "capture lock poisoned")?.clone())
}

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
                return;
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
                        if let Ok(mut recorded) = record.lock() {
                            recorded.push(buf.drain(..len).collect());
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

async fn drain(stream: &mut TcpStream) -> String {
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest).await;
    String::from_utf8_lossy(&rest).into_owned()
}

async fn raw_upstream(writes: Vec<Vec<u8>>, hold_open: bool) -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let writes = writes.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while complete_request_len(&buf).is_none() {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                for write in writes {
                    if stream.write_all(&write).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
                if hold_open {
                    let _ = stream.read(&mut chunk).await;
                }
            });
        }
    });
    Ok(port)
}

async fn read_until_quiet(stream: &mut TcpStream) -> Vec<u8> {
    let mut all = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match tokio::time::timeout(Duration::from_millis(400), stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return all,
            Ok(Ok(n)) => all.extend_from_slice(&chunk[..n]),
        }
    }
}

async fn serves_afterwards(h: &Harness) -> TestResult {
    let (port, _) = scripted_upstream(OK_RESPONSE).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    Ok(())
}

#[tokio::test]
async fn plain_http_answers_502_and_relays_nothing_of_an_invalid_upstream_head() -> TestResult {
    let cases: [(&str, &[u8]); 6] = [
        (
            "status 000",
            b"HTTP/1.1 000 Nope\r\nx-up: 1\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "status 099",
            b"HTTP/1.1 099 Nope\r\nx-up: 1\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "status 1000",
            b"HTTP/1.1 1000 Nope\r\nx-up: 1\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "control character in a header value",
            b"HTTP/1.1 200 OK\r\nx-up: 1\r\nx-bad: a\x01b\r\nContent-Length: 4\r\n\r\nleak",
        ),
        (
            "invalid header name",
            b"HTTP/1.1 200 OK\r\nx-up: 1\r\nbad name: v\r\nContent-Length: 4\r\n\r\nleak",
        ),
        ("not an http response", b"leak\r\n\r\nleak"),
    ];
    let h = start(IngressLimits::default()).await?;
    for (name, response) in cases {
        let port = raw_upstream(vec![response.to_vec()], true).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
        let answer = String::from_utf8_lossy(&read_until_quiet(&mut stream).await).into_owned();
        assert!(answer.starts_with("HTTP/1.1 502 "), "{name}: {answer}");
        assert!(!answer.contains("x-up"), "{name}: {answer}");
        assert!(!answer.contains("leak"), "{name}: {answer}");
    }
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_relays_exactly_the_declared_length_however_the_excess_arrives() -> TestResult {
    let cases: [(&str, Vec<Vec<u8>>); 2] = [
        (
            "in separate writes",
            vec![
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nwhole".to_vec(),
                b"EXCESS".to_vec(),
            ],
        ),
        (
            "in one write",
            vec![b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nwholeEXCESS".to_vec()],
        ),
    ];
    let h = start(IngressLimits::default()).await?;
    for (name, writes) in cases {
        let port = raw_upstream(writes, true).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
        let answer = String::from_utf8_lossy(&read_until_quiet(&mut stream).await).into_owned();
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{name}: {answer}");
        assert!(answer.ends_with("\r\n\r\nwhole"), "{name}: {answer}");
        assert!(!answer.contains("EXCESS"), "{name}: {answer}");
    }
    Ok(())
}

#[tokio::test]
async fn plain_http_never_pads_a_short_upstream_body_to_its_declared_length() -> TestResult {
    let port = raw_upstream(
        vec![b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nwhole".to_vec()],
        false,
    )
    .await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let answer = String::from_utf8_lossy(&read_until_quiet(&mut stream).await).into_owned();
    assert!(answer.contains("content-length: 9\r\n"), "{answer}");
    assert!(answer.ends_with("\r\n\r\nwhole"), "{answer}");
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_does_not_complete_a_chunked_response_whose_upstream_chunk_is_malformed() -> TestResult {
    let port = raw_upstream(
        vec![
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
            b"zz\r\nbad\r\n".to_vec(),
        ],
        true,
    )
    .await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
    let answer = String::from_utf8_lossy(&read_until_quiet(&mut stream).await).into_owned();
    assert!(!answer.ends_with("0\r\n\r\n"), "{answer}");
    assert!(!answer.starts_with("HTTP/1.1 502"), "{answer}");
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_survives_a_client_that_leaves_while_the_upstream_is_slow() -> TestResult {
    let port = raw_upstream(
        vec![b"".to_vec(), b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()],
        false,
    )
    .await?;
    let mut h = start(IngressLimits::default()).await?;
    let mut clients = Vec::new();
    for _ in 0..15 {
        let mut stream = TcpStream::connect(h.addr).await?;
        stream.write_all(proxied("GET", port, "/", "", "").as_bytes()).await?;
        clients.push(stream);
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    drop(clients);
    for _ in 0..15 {
        tokio::time::timeout(Duration::from_secs(5), h.outcomes.recv())
            .await?
            .ok_or("expected a value")?;
    }
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_aborted_chunked_upload_never_reaches_the_upstream_complete() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let head = proxied("POST", port, "/up", "Transfer-Encoding: chunked\r\n", "");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(b"5\r\nhello\r\n").await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(seen_requests(&seen)?.is_empty());
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_abort_mid_upload_to_a_blocked_host_leaves_the_proxy_serving() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let head = format!(
        "POST http://blocked.test:{port}/up HTTP/1.1\r\nHost: blocked.test:{port}\r\nProxy-Authorization: {}\r\nContent-Length: 1000000\r\n\r\n",
        basic(TOKEN)
    );
    stream.write_all(head.as_bytes()).await?;
    let _ = stream.write_all(&[b'a'; 4096]).await;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(seen_requests(&seen)?.is_empty());
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_answers_garbage_with_a_400_or_closes_promptly() -> TestResult {
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream.write_all(b"GARBAGE REQUEST LINE\r\n\r\n").await?;
    let answer = tokio::time::timeout(Duration::from_secs(5), drain(&mut stream)).await?;
    assert!(answer.is_empty() || answer.starts_with("HTTP/1.1 400 "), "{answer}");
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_gives_a_get_its_body_to_the_upstream_when_nothing_asked_to_see_it() -> TestResult {
    let cases: [(&str, &str, &str); 3] = [
        ("content-length zero", "Content-Length: 0\r\n", ""),
        ("content-length body", "Content-Length: 4\r\n", "data"),
        (
            "chunked body",
            "Transfer-Encoding: chunked\r\n",
            "4\r\ndata\r\n0\r\n\r\n",
        ),
    ];
    for (name, extra, body) in cases {
        let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
        let h = start(IngressLimits::default()).await?;
        let mut stream = TcpStream::connect(h.addr).await?;
        stream
            .write_all(proxied("GET", port, "/", extra, body).as_bytes())
            .await?;
        let (head, answer) = read_response(&mut stream).await?;
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{name}: {head}");
        assert_eq!(answer, "ok", "{name}");
        let first = seen_requests(&seen)?
            .into_iter()
            .next()
            .ok_or("the upstream saw no request")?;
        let sent = String::from_utf8(first)?;
        assert!(sent.ends_with(body), "{name}: {sent}");
    }
    Ok(())
}

async fn plain_sent(extra: &str, body: &str) -> TestResult<String> {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(proxied("POST", port, "/", extra, body).as_bytes())
        .await?;
    let (head, _) = read_response(&mut stream).await?;
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let first = seen_requests(&seen)?
        .into_iter()
        .next()
        .ok_or("the upstream saw no request")?;
    Ok(String::from_utf8(first)?)
}

#[tokio::test]
async fn plain_http_rebuilds_a_host_that_connection_names() -> TestResult {
    let sent = plain_sent("Connection: Host\r\n", "").await?;
    let lowered = sent.to_ascii_lowercase();
    assert_eq!(lowered.matches("\r\nhost: allowed.test:").count(), 1, "{sent}");
    assert_eq!(lowered.matches("connection:").count(), 1, "{sent}");
    assert!(lowered.ends_with("\r\nconnection: close\r\n\r\n"), "{sent}");
    Ok(())
}

#[tokio::test]
async fn plain_http_chunks_a_body_whose_content_length_connection_names() -> TestResult {
    let sent = plain_sent("Connection: Content-Length\r\nContent-Length: 5\r\n", "hello").await?;
    let lowered = sent.to_ascii_lowercase();
    assert!(!lowered.contains("content-length"), "{sent}");
    assert!(lowered.contains("transfer-encoding: chunked\r\n"), "{sent}");
    assert!(sent.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"), "{sent}");
    Ok(())
}

#[tokio::test]
async fn plain_http_keeps_a_chunked_body_whose_transfer_encoding_connection_names_chunked() -> TestResult {
    let sent = plain_sent(
        "Connection: Transfer-Encoding\r\nTransfer-Encoding: chunked\r\n",
        "5\r\nhello\r\n0\r\n\r\n",
    )
    .await?;
    let lowered = sent.to_ascii_lowercase();
    assert_eq!(lowered.matches("transfer-encoding:").count(), 1, "{sent}");
    assert!(sent.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"), "{sent}");
    Ok(())
}

#[tokio::test]
async fn plain_http_ignores_an_empty_transfer_encoding_value() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    stream
        .write_all(proxied("GET", port, "/", "Transfer-Encoding:\r\n", "").as_bytes())
        .await?;
    let answer = read_until_quiet(&mut stream).await;
    let answer = String::from_utf8_lossy(&answer);
    assert!(
        answer.starts_with("HTTP/1.1 200 OK") || answer.starts_with("HTTP/1.1 400 "),
        "{answer}"
    );
    if let Some(sent) = seen_requests(&seen)?.first() {
        let sent = String::from_utf8_lossy(sent).to_ascii_lowercase();
        assert!(!sent.contains("chunked"), "{sent}");
    }
    serves_afterwards(&h).await?;
    Ok(())
}

#[tokio::test]
async fn plain_http_chunked_get_cannot_smuggle_a_second_request() -> TestResult {
    let (port, seen) = scripted_upstream(OK_RESPONSE).await?;
    let h = start(IngressLimits::default()).await?;
    let mut stream = TcpStream::connect(h.addr).await?;
    let smuggled = format!(
        "POST http://allowed.test:{port}/smuggled HTTP/1.1\r\nHost: allowed.test:{port}\r\nContent-Length: 0\r\n\r\n"
    );
    let body = format!("0\r\n\r\n{smuggled}");
    stream
        .write_all(proxied("GET", port, "/", "Transfer-Encoding: chunked\r\n", &body).as_bytes())
        .await?;
    let _ = read_until_quiet(&mut stream).await;
    let seen = seen_requests(&seen)?;
    assert!(
        seen.iter()
            .all(|request| !String::from_utf8_lossy(request).contains("/smuggled")
                || String::from_utf8_lossy(request).starts_with("POST http")),
    );
    assert!(seen.len() <= 2);
    Ok(())
}

#[allow(deprecated)]
#[tokio::test]
async fn plain_http_rst_mid_download_leaves_the_proxy_serving() -> TestResult {
    let big = vec![b'x'; 8 * 1024 * 1024];
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", big.len()).into_bytes();
    response.extend_from_slice(&big);
    let port = raw_upstream(vec![response], false).await?;
    let h = start(IngressLimits::default()).await?;
    let stream = TcpStream::connect(h.addr).await?;
    let mut stream = stream;
    stream
        .write_all(proxied("GET", port, "/download", "", "").as_bytes())
        .await?;
    let mut got = vec![0u8; 256 * 1024];
    stream.read_exact(&mut got).await?;
    stream.set_linger(Some(Duration::ZERO))?;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    serves_afterwards(&h).await?;
    Ok(())
}
