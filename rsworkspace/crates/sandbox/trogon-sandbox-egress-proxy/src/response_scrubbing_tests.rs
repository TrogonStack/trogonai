use std::io;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tracing_subscriber::fmt::MakeWriter;
use trogon_sandbox_contract::{DomainPattern, InjectHosts, SecretName};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

use crate::framed_head::undated;
use crate::http_intercept::{BodyLimit, RequestGuard, TlsTarget, run_compiled_http_intercept};
use crate::secret_substitution::tests::{API_REAL, Fixture, REGISTRY_REAL, fixture};
use crate::secret_substitution::{CompressedResponsePolicy, SecretSubstitution, SentinelMisdirectionObserver};
use crate::substitution_tests::{TestResult, lock};
use crate::upstream_leg::UpstreamLeg;

const API_HOST: &str = "api.example.com";
const OTHER_HOST: &str = "example.net";

#[derive(Default)]
struct Compressed(Mutex<Vec<(String, u16, String)>>);

#[async_trait]
impl SentinelMisdirectionObserver for Compressed {
    async fn sentinel_off_host(&self, _host: &str, _port: u16, _binding: &SecretName) {}

    async fn compressed_response(&self, host: &str, port: u16, binding: &SecretName) {
        if let Ok(mut seen) = self.0.lock() {
            seen.push((host.to_string(), port, binding.to_string()));
        }
    }
}

struct Outcome {
    upstream: Vec<u8>,
    client: Vec<u8>,
    served: usize,
}

fn guard(substitution: &SecretSubstitution, host: &str) -> RequestGuard {
    let target = TlsTarget::new(host, 443);
    RequestGuard::new(target.clone()).with_substitution(substitution.scope_for(&target))
}

fn get(host: &str) -> Vec<u8> {
    format!("GET /x HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()
}

async fn relay(guard: RequestGuard, requests: &[Vec<u8>], responses: Vec<Vec<Vec<u8>>>) -> TestResult<Outcome> {
    let (mut client_side, proxy_client) = duplex(256 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(256 * 1024);

    let upstream = tokio::spawn(async move {
        let mut seen = Vec::new();
        let mut served = 0;
        let mut buf = vec![0u8; 64 * 1024];
        for pieces in responses {
            while seen.windows(4).filter(|w| *w == b"\r\n\r\n").count() <= served {
                let n = upstream_side.read(&mut buf).await?;
                if n == 0 {
                    return io::Result::Ok((seen, served));
                }
                seen.extend_from_slice(&buf[..n]);
            }
            for piece in pieces {
                if piece.is_empty() {
                    let _ = upstream_side.shutdown().await;
                    return io::Result::Ok((seen, served + 1));
                }
                upstream_side.write_all(&piece).await?;
                upstream_side.flush().await?;
                tokio::task::yield_now().await;
            }
            served += 1;
        }
        let _ = upstream_side.shutdown().await;
        io::Result::Ok((seen, served))
    });

    for request in requests {
        client_side.write_all(request).await?;
    }
    client_side.shutdown().await?;

    run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), None, &guard).await?;

    let mut client = Vec::new();
    client_side.read_to_end(&mut client).await?;
    let (upstream, served) = upstream.await??;
    Ok(Outcome {
        upstream,
        client,
        served,
    })
}

async fn single(guard: RequestGuard, pieces: Vec<Vec<u8>>) -> TestResult<Outcome> {
    relay(guard, &[get(API_HOST)], vec![pieces]).await
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn whole(response: &str) -> Vec<Vec<u8>> {
    vec![response.as_bytes().to_vec()]
}

fn split_every(response: &str, size: usize) -> Vec<Vec<u8>> {
    response.as_bytes().chunks(size).map(<[u8]>::to_vec).collect()
}

fn then_close(mut pieces: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    pieces.push(Vec::new());
    pieces
}

fn decode_chunked(body: &str) -> TestResult<(String, String)> {
    let mut decoded = String::new();
    let mut rest = body;
    loop {
        let (size, after) = rest.split_once("\r\n").ok_or("expected a chunk size")?;
        let size = usize::from_str_radix(size, 16)?;
        if size == 0 {
            return Ok((decoded, after.to_string()));
        }
        decoded.push_str(after.get(..size).ok_or("chunk shorter than its size")?);
        assert_eq!(after.get(size..size + 2), Some("\r\n"));
        rest = after.get(size + 2..).ok_or("chunk missing its terminator")?;
    }
}

fn chunked_body(client: &str) -> TestResult<String> {
    let (head, body) = client.split_once("\r\n\r\n").ok_or("expected a header terminator")?;
    assert!(head.contains("Transfer-Encoding: chunked"), "{head}");
    let (decoded, trailers) = decode_chunked(body)?;
    assert_eq!(trailers, "\r\n");
    Ok(decoded)
}

fn sized(headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn assert_clean(output: &str) {
    assert!(!output.contains(API_REAL), "api real value leaked");
    assert!(!output.contains(REGISTRY_REAL), "registry real value leaked");
}

#[tokio::test]
async fn a_reflected_header_and_body_are_scrubbed_and_content_length_follows() -> Result<(), Box<dyn std::error::Error>>
{
    let fx = fixture()?;
    assert_ne!(fx.api.len(), API_REAL.len());
    let body = format!("{{\"echo\":\"{API_REAL}\"}}");
    let response = sized(&format!("X-Echo: Bearer {API_REAL}\r\n"), &body);

    let outcome = single(guard(&fx.substitution, API_HOST), whole(&response)).await?;

    let client = text(&outcome.client);
    assert_clean(&client);
    let expected_body = format!("{{\"echo\":\"{}\"}}", fx.api);
    assert_eq!(
        undated(&client),
        format!(
            "HTTP/1.1 200 OK\r\nx-echo: Bearer {}\r\ncontent-length: {}\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\n{expected_body}",
            fx.api,
            expected_body.len()
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_value_split_at_every_read_boundary_is_still_scrubbed() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = sized(
        &format!("X-Echo: {API_REAL}\r\n"),
        &format!("a{API_REAL}b{REGISTRY_REAL}c"),
    );

    for size in [1, 2, 3, 5, 7, 11, 64] {
        let outcome = single(guard(&fx.substitution, API_HOST), split_every(&response, size)).await?;

        let client = text(&outcome.client);
        assert_clean(&client);
        assert!(
            client.ends_with(&format!("a{}b{}c", fx.api, fx.registry)),
            "size {size}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn real_values_of_bindings_for_other_hosts_are_scrubbed_too() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = sized("", REGISTRY_REAL);

    let outcome = single(guard(&fx.substitution, API_HOST), whole(&response)).await?;

    assert!(text(&outcome.client).ends_with(&fx.registry));
    Ok(())
}

#[tokio::test]
async fn a_chunked_response_is_rechunked_with_a_value_split_across_chunks_and_scrubbed_trailers()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let (first, second) = API_REAL.split_at(9);
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x};ext=1\r\nk={first}\r\n{:x}\r\n{second}!\r\n0\r\nX-Echo: {API_REAL}\r\n\r\n",
        first.len() + 2,
        second.len() + 1,
    );

    let outcome = single(guard(&fx.substitution, API_HOST), split_every(&response, 6)).await?;

    let client = text(&outcome.client);
    assert_clean(&client);
    let (head, body) = client.split_once("\r\n\r\n").ok_or("expected a value")?;
    assert!(head.contains("Transfer-Encoding: chunked"));
    let (decoded, trailers) = decode_chunked(body)?;
    assert_eq!(decoded, format!("k={}!", fx.api));
    assert_eq!(trailers, "\r\n");
    Ok(())
}

#[tokio::test]
async fn a_chunked_response_with_a_content_length_loses_the_length() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: 999\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{API_REAL}\r\n0\r\n\r\n",
        API_REAL.len()
    );

    let outcome = single(guard(&fx.substitution, API_HOST), whole(&response)).await?;

    let client = text(&outcome.client);
    assert!(!client.to_ascii_lowercase().contains("content-length"));
    let (_, body) = client.split_once("\r\n\r\n").ok_or("expected a value")?;
    assert_eq!(decode_chunked(body)?.0, fx.api);
    Ok(())
}

#[tokio::test]
async fn head_204_and_304_responses_have_no_body_to_wait_for() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let head_request = b"HEAD /x HTTP/1.1\r\nHost: api.example.com\r\n\r\n".to_vec();

    let head = relay(
        guard(&fx.substitution, API_HOST),
        &[head_request],
        vec![whole(&format!(
            "HTTP/1.1 200 OK\r\nX-Echo: {API_REAL}\r\nContent-Length: 500\r\n\r\n"
        ))],
    )
    .await?;
    let head_text = text(&head.client);
    assert!(head_text.contains(&format!("x-echo: {}\r\n", fx.api)));
    assert!(head_text.contains("content-length: 500\r\n"));
    assert_clean(&head_text);

    for status in ["204 No Content", "304 Not Modified"] {
        let outcome = single(
            guard(&fx.substitution, API_HOST),
            whole(&format!("HTTP/1.1 {status}\r\nETag: \"{API_REAL}\"\r\n\r\n")),
        )
        .await?;
        let client = text(&outcome.client);
        assert!(client.contains(&format!("etag: \"{}\"\r\n", fx.api)), "{status}");
        assert_clean(&client);
    }
    Ok(())
}

#[tokio::test]
async fn an_interim_100_continue_is_relayed_before_the_final_response() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = format!("HTTP/1.1 100 Continue\r\n\r\n{}", sized("", &format!("v={API_REAL}")));

    let outcome = single(guard(&fx.substitution, API_HOST), split_every(&response, 9)).await?;

    let client = text(&outcome.client);
    assert!(client.starts_with("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n"));
    assert!(client.ends_with(&format!("v={}", fx.api)));
    Ok(())
}

#[tokio::test]
async fn a_close_delimited_body_is_scrubbed_and_ends_the_connection() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nraw {API_REAL} tail");

    let outcome = relay(
        guard(&fx.substitution, API_HOST),
        &[get(API_HOST), get(API_HOST)],
        vec![
            then_close(split_every(&response, 4)),
            whole("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"),
        ],
    )
    .await?;

    let client = text(&outcome.client);
    assert_eq!(chunked_body(&client)?, format!("raw {} tail", fx.api));
    assert!(!client.contains("Connection: close"));
    assert_eq!(client.matches("HTTP/1.1 200").count(), 1);
    assert_eq!(outcome.served, 1);
    assert_clean(&client);
    Ok(())
}

#[tokio::test]
async fn a_body_without_framing_runs_to_the_end_of_the_connection() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = format!("HTTP/1.1 200 OK\r\n\r\n{API_REAL}");

    let outcome = single(guard(&fx.substitution, API_HOST), then_close(split_every(&response, 8))).await?;

    assert_eq!(chunked_body(&text(&outcome.client))?, fx.api);
    Ok(())
}

#[tokio::test]
async fn keep_alive_responses_are_relayed_one_after_another() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;

    let outcome = relay(
        guard(&fx.substitution, API_HOST),
        &[get(API_HOST), get(API_HOST)],
        vec![whole(&sized("", API_REAL)), whole(&sized("", &format!("<{API_REAL}>")))],
    )
    .await?;

    let client = text(&outcome.client);
    assert_eq!(outcome.served, 2);
    assert!(client.contains(&fx.api));
    assert!(client.ends_with(&format!("<{}>", fx.api)));
    assert_clean(&client);
    Ok(())
}

#[tokio::test]
async fn a_body_past_the_buffer_limit_is_streamed_as_chunked() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let body = format!("{}{API_REAL}{}", "x".repeat(40), "y".repeat(40));
    let response = sized("", &body);
    let limited = guard(&fx.substitution, API_HOST).with_body_limit(BodyLimit::new(16));

    let outcome = single(limited, split_every(&response, 13)).await?;

    let client = text(&outcome.client);
    assert_clean(&client);
    let (head, rest) = client.split_once("\r\n\r\n").ok_or("expected a value")?;
    assert!(!head.to_ascii_lowercase().contains("content-length"));
    assert!(head.contains("Transfer-Encoding: chunked"));
    assert_eq!(
        decode_chunked(rest)?.0,
        format!("{}{}{}", "x".repeat(40), fx.api, "y".repeat(40))
    );
    Ok(())
}

#[tokio::test]
async fn a_body_past_the_buffer_limit_from_an_http_1_0_upstream_is_close_delimited()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let body = format!("{}{API_REAL}", "x".repeat(40));
    let response = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let limited = guard(&fx.substitution, API_HOST).with_body_limit(BodyLimit::new(16));

    let outcome = single(limited, then_close(whole(&response))).await?;

    let client = text(&outcome.client);
    let (head, _) = client.split_once("\r\n\r\n").ok_or("expected a value")?;
    assert!(!head.to_ascii_lowercase().contains("content-length"));
    assert!(head.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(chunked_body(&client)?, format!("{}{}", "x".repeat(40), fx.api));
    Ok(())
}

#[tokio::test]
async fn a_compressed_response_from_an_inject_host_is_refused_and_reported_by_default()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let observer = Arc::new(Compressed::default());
    let substitution = fx.substitution.clone().with_observer(observer.clone());
    let response = sized("Content-Encoding: gzip\r\n", &format!("opaque {API_REAL}"));

    let outcome = single(guard(&substitution, API_HOST), whole(&response)).await?;

    let client = text(&outcome.client);
    assert!(client.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
    assert!(client.contains("X-Proxy-Error: compressed-response\r\n"));
    assert_clean(&client);
    assert_eq!(
        *lock(&observer.0)?,
        vec![(API_HOST.to_string(), 443, "api".to_string())]
    );
    Ok(())
}

#[tokio::test]
async fn a_compressed_response_is_forwarded_and_reported_when_the_policy_flags()
-> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let observer = Arc::new(Compressed::default());
    let substitution = fx
        .substitution
        .clone()
        .with_observer(observer.clone())
        .with_compressed_response_policy(CompressedResponsePolicy::Flag);
    let response = sized("Content-Encoding: br\r\nX-Echo: x\r\n", "opaque-bytes");

    let outcome = single(guard(&substitution, API_HOST), whole(&response)).await?;

    assert_eq!(
        undated(&text(&outcome.client)),
        "HTTP/1.1 200 OK\r\ncontent-encoding: br\r\nx-echo: x\r\ncontent-length: 12\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nopaque-bytes"
    );
    assert_eq!(lock(&observer.0)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_compressed_response_with_no_body_is_not_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let request = b"HEAD /x HTTP/1.1\r\nHost: api.example.com\r\n\r\n".to_vec();

    let outcome = relay(
        guard(&fx.substitution, API_HOST),
        &[request],
        vec![whole(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 20\r\n\r\n",
        )],
    )
    .await?;

    assert!(text(&outcome.client).starts_with("HTTP/1.1 200 OK"));
    Ok(())
}

#[tokio::test]
async fn an_identity_content_encoding_is_scrubbed_like_no_encoding() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = sized("Content-Encoding: identity\r\n", API_REAL);

    let outcome = single(guard(&fx.substitution, API_HOST), whole(&response)).await?;

    assert!(text(&outcome.client).ends_with(&fx.api));
    Ok(())
}

#[tokio::test]
async fn accept_encoding_is_narrowed_to_identity_at_an_inject_host_only() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let request =
        |host: &str| format!("GET /x HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: gzip, br\r\n\r\n").into_bytes();
    let ok = || whole("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");

    let inject = relay(guard(&fx.substitution, API_HOST), &[request(API_HOST)], vec![ok()]).await?;
    let other = relay(guard(&fx.substitution, OTHER_HOST), &[request(OTHER_HOST)], vec![ok()]).await?;

    assert!(text(&inject.upstream).contains("Accept-Encoding: identity\r\n"));
    assert!(text(&other.upstream).contains("Accept-Encoding: gzip, br\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_host_no_binding_injects_at_gets_its_compressed_response_through() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let response = sized("Content-Encoding: gzip\r\n", "opaque");

    let outcome = relay(
        guard(&fx.substitution, OTHER_HOST),
        &[get(OTHER_HOST)],
        vec![whole(&response)],
    )
    .await?;

    assert_eq!(
        undated(&text(&outcome.client)),
        "HTTP/1.1 200 OK\r\ncontent-encoding: gzip\r\ncontent-length: 6\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nopaque"
    );
    Ok(())
}

#[tokio::test]
async fn conflicting_response_lengths_get_a_502_without_forwarding_a_body() -> Result<(), Box<dyn std::error::Error>> {
    let fx = fixture()?;
    let (mut client_side, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(64 * 1024);
    client_side.write_all(&get(API_HOST)).await?;
    client_side.shutdown().await?;
    upstream_side
        .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\n{API_REAL}").as_bytes())
        .await?;

    let result = run_compiled_http_intercept(
        proxy_client,
        UpstreamLeg::connected(proxy_upstream),
        None,
        &guard(&fx.substitution, API_HOST),
    )
    .await;

    assert!(result.is_ok());
    let mut client = Vec::new();
    client_side.read_to_end(&mut client).await?;
    assert!(text(&client).starts_with("HTTP/1.1 502 "));
    assert_clean(&text(&client));
    Ok(())
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

fn lock_io<T>(mutex: &Mutex<T>) -> io::Result<std::sync::MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| io::Error::other("poisoned lock"))
}

impl io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock_io(&self.0)?.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn tracing_and_debug_output_never_contain_a_real_value() -> Result<(), Box<dyn std::error::Error>> {
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();

    let fx: Fixture = fixture()?;
    let observer = Arc::new(Compressed::default());
    let substitution = fx.substitution.clone().with_observer(observer);

    let reflected = sized(
        &format!("X-Echo: {API_REAL}\r\n"),
        &format!("{API_REAL}{REGISTRY_REAL}"),
    );
    let compressed = sized("Content-Encoding: gzip\r\n", API_REAL);
    let mut captured = String::new();
    for _ in 0..50 {
        tracing::callsite::rebuild_interest_cache();
        single(guard(&substitution, API_HOST), whole(&reflected)).await?;
        single(guard(&substitution, API_HOST), whole(&compressed)).await?;
        let tiny = guard(&substitution, API_HOST).with_body_limit(BodyLimit::new(4));
        single(tiny, whole(&reflected)).await?;
        captured = text(&lock(&logs.0)?);
        if captured.contains("reflected secret values") && captured.contains("cannot be scanned") {
            break;
        }
    }
    assert!(captured.contains("reflected secret values"));
    assert!(captured.contains("cannot be scanned"));
    assert_clean(&captured);
    let printed = format!("{substitution:?} {:?}", guard(&substitution, API_HOST));
    assert_clean(&printed);
    Ok(())
}

#[tokio::test]
async fn a_buffered_body_that_grows_past_the_limit_when_scrubbed_is_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let fx = fixture()?;
    assert!(fx.api.len() > API_REAL.len());
    let body = API_REAL.repeat(4);
    let response = sized("", &body);
    let limited = guard(&fx.substitution, API_HOST).with_body_limit(BodyLimit::new(body.len()));

    let outcome = single(limited, whole(&response)).await?;

    let client = text(&outcome.client);
    assert!(client.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
    assert!(client.contains("X-Proxy-Error: scrubbed-response-too-large\r\n"));
    assert_clean(&client);
    Ok(())
}

#[tokio::test]
async fn a_value_too_short_to_scrub_safely_is_left_in_responses() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    registry.register(
        SecretName::new("pin")?,
        RealValue::new("ab".to_string()),
        InjectHosts::new(vec![DomainPattern::new(API_HOST)?])?,
    );
    let substitution = SecretSubstitution::new(Arc::new(registry));
    let response = sized("", "a table of abacus and abbey");

    let outcome = single(guard(&substitution, API_HOST), whole(&response)).await?;

    assert!(text(&outcome.client).ends_with("a table of abacus and abbey"));
    Ok(())
}
