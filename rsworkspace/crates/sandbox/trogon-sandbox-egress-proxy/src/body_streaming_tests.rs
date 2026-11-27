//! Request bodies are piped to the upstream as they arrive, with no size
//! limit of their own.

use std::io;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

use crate::http_intercept::{RequestGuard, TlsTarget, run_compiled_http_intercept};
use crate::request_decision::RequestDenial;
use crate::request_filter::{RequestChecks, RequestDecision, RequestFilter, RequestView};
use crate::secret_substitution::tests::{API_REAL, fixture, scope as scope_for};
use crate::upstream_leg::UpstreamLeg;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

const HOST: &str = "api.example.com";
const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
const NINE_MIB: usize = 9 << 20;

struct Relayed {
    upstream: Vec<u8>,
    client: Vec<u8>,
}

async fn read_request_upstream<R: AsyncReadExt + Unpin>(side: &mut R) -> io::Result<Vec<u8>> {
    let mut seen = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut needed: Option<usize> = None;
    loop {
        if needed.is_none()
            && let Some(head_end) = seen.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(&seen[..head_end]).to_ascii_lowercase();
            let body = match head.lines().find_map(|line| line.strip_prefix("content-length:")) {
                Some(length) => Some(length.trim().parse::<usize>().map_err(io::Error::other)?),
                None => None,
            };
            needed = Some(match body {
                Some(length) => head_end + 4 + length,
                None => usize::MAX,
            });
        }
        let done = match needed {
            Some(usize::MAX) => seen.ends_with(b"0\r\n\r\n"),
            Some(total) => seen.len() >= total,
            None => false,
        };
        if done {
            return Ok(seen);
        }
        let n = side.read(&mut chunk).await?;
        if n == 0 {
            return Ok(seen);
        }
        seen.extend_from_slice(&chunk[..n]);
    }
}

async fn relay(guard: RequestGuard, head: &str, body: &[u8], wait_for_continue: bool) -> TestResult<Relayed> {
    let (mut client_side, proxy_client) = duplex(256 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(256 * 1024);

    let upstream = async move {
        let seen = read_request_upstream(&mut upstream_side).await?;
        if !seen.is_empty() {
            let _ = upstream_side.write_all(OK).await;
        }
        io::Result::Ok(seen)
    };
    let client = async {
        client_side.write_all(head.as_bytes()).await?;
        if wait_for_continue {
            let mut interim = [0u8; 25];
            client_side.read_exact(&mut interim).await?;
            assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        for piece in body.chunks(64 * 1024) {
            if client_side.write_all(piece).await.is_err() {
                break;
            }
        }
        let _ = client_side.shutdown().await;
        let mut answer = Vec::new();
        client_side.read_to_end(&mut answer).await?;
        io::Result::Ok(answer)
    };
    let proxy = run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), None, &guard);
    let (upstream, client, proxied) = tokio::join!(upstream, client, proxy);
    proxied?;
    Ok(Relayed {
        upstream: upstream?,
        client: client?,
    })
}

fn head(extra: &str, length: usize) -> String {
    format!("POST /upload HTTP/1.1\r\nHost: {HOST}\r\n{extra}Content-Length: {length}\r\n\r\n")
}

fn plain_guard() -> RequestGuard {
    RequestGuard::new(TlsTarget::new(HOST, 443))
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[tokio::test]
async fn a_body_past_the_default_cap_is_relayed_whole() -> TestResult<()> {
    let body = vec![b'x'; NINE_MIB];

    let relayed = relay(plain_guard(), &head("", body.len()), &body, false).await?;

    let head_len = relayed
        .upstream
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("expected a value")?
        + 4;
    assert_eq!(relayed.upstream.len() - head_len, NINE_MIB);
    assert!(relayed.client.starts_with(b"HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn a_body_past_the_default_cap_is_substituted_as_it_streams() -> TestResult<()> {
    let fx = fixture()?;
    let sentinel = fx.api.clone();
    let mut body = vec![b'.'; NINE_MIB];
    for at in [0, NINE_MIB / 2 + 3, NINE_MIB - sentinel.len()] {
        body[at..at + sentinel.len()].copy_from_slice(sentinel.as_bytes());
    }
    let guard = plain_guard().with_substitution(Some(scope_for(&fx, HOST)?));

    let relayed = relay(guard, &head("", body.len()), &body, false).await?;

    assert_eq!(occurrences(&relayed.upstream, API_REAL.as_bytes()), 3);
    assert_eq!(occurrences(&relayed.upstream, b"fake_value_"), 0);
    assert!(relayed.client.starts_with(b"HTTP/1.1 200"));
    Ok(())
}

struct Reads {
    wanted: usize,
    seen: Mutex<Vec<u8>>,
    verdict: fn(&[u8]) -> RequestDecision,
}

#[async_trait]
impl RequestFilter for Reads {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        let Some(body) = request.body else {
            return RequestDecision::DenyWith(RequestDenial::new());
        };
        let Ok(read) = body.read(self.wanted).await else {
            return RequestDecision::DenyWith(RequestDenial::new());
        };
        if let Ok(mut seen) = self.seen.lock() {
            *seen = read.bytes.clone();
        }
        (self.verdict)(&read.bytes)
    }
}

fn guard_with(filter: Arc<Reads>) -> RequestGuard {
    plain_guard().with_request_checks(Some(RequestChecks::new(filter)))
}

#[tokio::test]
async fn a_filter_reads_the_body_on_demand_and_the_whole_body_is_still_forwarded() -> TestResult<()> {
    let filter = Arc::new(Reads {
        wanted: 5,
        seen: Mutex::default(),
        verdict: |_| RequestDecision::Allow,
    });
    let body = vec![b'q'; 300_000];

    let relayed = relay(guard_with(Arc::clone(&filter)), &head("", body.len()), &body, false).await?;

    assert_eq!(*filter.seen.lock().map_err(|_| "poisoned lock")?, b"qqqqq");
    assert!(relayed.upstream.ends_with(&body));
    assert!(relayed.client.starts_with(b"HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn a_filter_that_denies_on_the_body_stops_the_request_before_the_upstream_sees_it() -> TestResult<()> {
    let filter = Arc::new(Reads {
        wanted: 4,
        seen: Mutex::default(),
        verdict: |bytes| {
            if bytes == b"evil" {
                RequestDecision::DenyWith(RequestDenial::new())
            } else {
                RequestDecision::Allow
            }
        },
    });
    let body = [b"evil".as_slice(), &vec![b'z'; 100_000]].concat();

    let relayed = relay(guard_with(filter), &head("", body.len()), &body, false).await?;

    assert!(relayed.upstream.is_empty());
    assert!(relayed.client.starts_with(b"HTTP/1.1 403"));
    Ok(())
}

#[tokio::test]
async fn an_expect_continue_client_is_told_to_send_the_body() -> TestResult<()> {
    let body = vec![b'e'; 1000];

    let relayed = relay(
        plain_guard(),
        &head("Expect: 100-continue\r\n", body.len()),
        &body,
        true,
    )
    .await?;

    assert!(relayed.upstream.ends_with(&body));
    assert!(relayed.client.starts_with(b"HTTP/1.1 200"));
    Ok(())
}

#[tokio::test]
async fn a_chunk_that_is_not_followed_by_crlf_ends_the_tunnel_with_nothing_forwarded() -> TestResult<()> {
    let request =
        format!("POST /upload HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXX0\r\n\r\n");

    let relayed = relay(plain_guard(), &request, &[], false).await?;

    assert_eq!(relayed.client, b"");
    assert_eq!(relayed.upstream, b"");
    Ok(())
}
