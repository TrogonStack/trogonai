use super::*;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::duplex;
use trogon_sandbox_contract::{CredentialProvider, CredentialRequest, CredentialScope, HeaderName, PullSecretId};

use crate::credential_source::tests::FakeCredentialSource;
use crate::held_writes::{HeldWrites, WAIT, expect_bytes};
use crate::request_decision::HeaderEdits;
use crate::upstream_leg::UpstreamLeg;

const HOST: &str = "api.example.com";

fn locked<T: Clone>(value: &std::sync::Mutex<T>) -> Result<T, Box<dyn std::error::Error>> {
    Ok(value.lock().map_err(|_| "a test lock was poisoned")?.clone())
}

fn guard() -> RequestGuard {
    RequestGuard::new(TlsTarget::new(HOST, 443))
}

#[tokio::test(start_paused = true)]
async fn a_client_quiet_past_the_keep_alive_time_is_not_waited_for() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(1024);
    let guard = guard().with_keep_alive(Duration::from_secs(6));
    let mut buf = Vec::new();
    let started = tokio::time::Instant::now();
    let next = guard.next_head(&mut server, &mut buf, Some(started)).await;
    assert_eq!(next, NextHead::Idle);
    assert_eq!(started.elapsed(), Duration::from_secs(6));
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_request_inside_the_keep_alive_time_continues_the_connection() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(1024);
    let guard = guard().with_keep_alive(Duration::from_secs(6));
    let head = b"GET / HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    client.write_all(head).await?;
    let mut buf = Vec::new();
    let since = Some(tokio::time::Instant::now());
    assert_eq!(guard.next_head(&mut server, &mut buf, since).await, NextHead::Arrived);
    assert_eq!(buf, head);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn part_of_a_head_does_not_move_the_keep_alive_time() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(1024);
    let guard = guard().with_keep_alive(Duration::from_secs(6));
    let since = tokio::time::Instant::now();
    let mut buf = Vec::new();
    let waiting = guard.next_head(&mut server, &mut buf, Some(since));
    let trickle = async {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            client.write_all(b"G").await?;
        }
        Ok::<(), std::io::Error>(())
    };
    let (next, trickled) = tokio::join!(waiting, trickle);
    trickled?;
    assert_eq!(next, NextHead::Idle);
    assert_eq!(since.elapsed(), Duration::from_secs(6));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn part_of_the_first_head_is_cut_off_at_the_first_request_time() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(1024);
    let hello_at = tokio::time::Instant::now();
    let guard = guard()
        .with_keep_alive(Duration::from_secs(6))
        .with_first_head(FirstHead::counted_from(hello_at, Duration::from_secs(10)));
    client.write_all(b"GET / HTTP/1.1\r\nX-Half").await?;
    let mut buf = Vec::new();
    assert_eq!(guard.next_head(&mut server, &mut buf, None).await, NextHead::Idle);
    assert_eq!(hello_at.elapsed(), Duration::from_secs(10));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_first_deadline_passed_during_the_handshake_moves_on_by_a_window() -> Result<(), Box<dyn std::error::Error>> {
    let hello_at = tokio::time::Instant::now();
    tokio::time::advance(Duration::from_secs(12)).await;
    let first = FirstHead::counted_from(hello_at, Duration::from_secs(10));
    assert_eq!(first.by, hello_at + Duration::from_secs(20));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_guard_without_a_keep_alive_never_cuts_a_connection() -> Result<(), Box<dyn std::error::Error>> {
    let (_client, mut server) = duplex(1024);
    let mut buf = Vec::new();
    let since = Some(tokio::time::Instant::now());
    assert_eq!(guard().next_head(&mut server, &mut buf, since).await, NextHead::Arrived);
    Ok(())
}

struct CountingObserver(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl crate::request_filter::DeniedRequestObserver for CountingObserver {
    async fn request_denied(&self, _host: &str, _port: u16, _request: RefusedRequest) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn checked_guard(
    rule: trogon_sandbox_contract::RequestRule,
) -> Result<(RequestGuard, Arc<CountingObserver>), Box<dyn std::error::Error>> {
    let policy = trogon_sandbox_contract::NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![rule],
    };
    let observer = Arc::new(CountingObserver(Default::default()));
    let filter = crate::request_filter::PolicyRequestFilter::from_policy(&policy).ok_or("expected a value")?;
    let checks = RequestChecks::new(Arc::new(filter)).with_observer(observer.clone());
    Ok((guard().with_request_checks(Some(checks)), observer))
}

fn deny_rule(
    methods: &[&str],
    prefix: Option<&str>,
) -> Result<trogon_sandbox_contract::RequestRule, Box<dyn std::error::Error>> {
    Ok(trogon_sandbox_contract::RequestRule {
        action: trogon_sandbox_contract::RequestRuleAction::Deny,
        domains: Vec::new(),
        methods: methods
            .iter()
            .map(|method| HttpMethod::new(*method))
            .collect::<Result<_, _>>()?,
        path_prefix: prefix.map(trogon_sandbox_contract::PathPrefix::new).transpose()?,
        headers: Vec::new(),
    })
}

async fn checked(guard: &RequestGuard, bytes: &[u8]) -> Result<(bool, String), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(bytes).await?;
    let mut buf = Vec::new();
    let parsed = read_checked_request(&mut server, &mut buf, guard).await?;
    drop(server);
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    Ok((parsed.is_some(), answer))
}

#[tokio::test]
async fn terminated_requests_are_checked_against_the_request_rules() -> Result<(), Box<dyn std::error::Error>> {
    let (guard, observer) = checked_guard(deny_rule(&["DELETE"], Some("/admin"))?)?;
    let (allowed, _) = checked(&guard, &get("/admin/x")).await?;
    assert!(allowed);
    let (allowed, _) = checked(&guard, b"DELETE /public HTTP/1.1\r\nHost: api.example.com\r\n\r\n").await?;
    assert!(allowed);
    assert_eq!(observer.0.load(std::sync::atomic::Ordering::SeqCst), 0);

    let (allowed, answer) = checked(&guard, b"DELETE /admin/x?y=1 HTTP/1.1\r\nHost: api.example.com\r\n\r\n").await?;
    assert!(!allowed);
    assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    assert!(answer.contains("denied by sandbox policy\n"), "{answer}");
    assert_eq!(observer.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

struct Fixed(RequestDecision);

#[async_trait::async_trait]
impl crate::request_filter::RequestFilter for Fixed {
    async fn decide(&self, _request: &RequestView<'_>) -> RequestDecision {
        self.0.clone()
    }
}

fn fixed_guard(decision: RequestDecision) -> RequestGuard {
    guard().with_request_checks(Some(RequestChecks::new(Arc::new(Fixed(decision)))))
}

#[tokio::test]
async fn a_deny_with_answers_its_status_reason_and_mark() -> Result<(), Box<dyn std::error::Error>> {
    let denial = RequestDenial::new()
        .status(429)
        .reason("slow down")
        .mark(crate::request_decision::DenyMark::parse("rate_limited")?);
    let guard = fixed_guard(RequestDecision::DenyWith(denial));
    let (allowed, answer) = checked(&guard, &get("/x")).await?;
    assert!(!allowed);
    assert_eq!(
        crate::framed_head::undated(&answer),
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\na\r\nslow down\n\r\n0\r\n\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn an_allow_with_edits_rewrites_the_forwarded_request() -> Result<(), Box<dyn std::error::Error>> {
    let edits = HeaderEdits::new().remove("X-Drop")?.set("X-Added", "1")?;
    let guard = fixed_guard(RequestDecision::AllowWith(RequestAllow::new().edits(edits)));
    let request = b"GET /x HTTP/1.1\r\nHost: api.example.com\r\nX-Drop: a\r\n\r\n".to_vec();
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(&request).await?;
    let mut buf = Vec::new();
    let parsed = read_checked_request(&mut server, &mut buf, &guard)
        .await?
        .ok_or("expected a value")?;
    let forwarded = String::from_utf8(parsed.raw_bytes().to_vec())?;
    assert!(forwarded.contains("x-added: 1\r\n"), "{forwarded}");
    assert!(!forwarded.to_lowercase().contains("x-drop"), "{forwarded}");
    assert!(forwarded.contains("Host: api.example.com"), "{forwarded}");
    Ok(())
}

/// A writer that, like a TLS stream whose socket was not writable, holds
/// what it is given until it is flushed.
#[derive(Default)]
struct HoldsUntilFlushed {
    held: Vec<u8>,
    sent: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl AsyncWrite for HoldsUntilFlushed {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.held.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let held = std::mem::take(&mut self.held);
        if let Ok(mut sent) = self.sent.lock() {
            sent.extend_from_slice(&held);
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

#[tokio::test]
async fn a_request_body_is_flushed_upstream_once_relayed() -> Result<(), Box<dyn std::error::Error>> {
    let body = vec![b'x'; 40_000];
    let mut request = format!(
        "POST /u HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(&request).await?;
    let mut buf = Vec::new();
    let CheckedRead::Request(parsed) =
        read_checked_request_as(&mut server, &mut buf, &guard(), BodyMode::Stream).await?
    else {
        return Err("the request was not handed over".into());
    };
    let mut upstream = HoldsUntilFlushed::default();
    let sent = Arc::clone(&upstream.sent);
    let relayed = relay_body_only(&mut server, &mut buf, &mut upstream, &parsed, None).await?;
    assert_eq!(relayed, body.len() as u64);
    assert_eq!(locked(&sent)?.len(), body.len());
    Ok(())
}

#[tokio::test]
async fn a_100_continue_is_flushed_to_the_client_before_its_body_is_awaited() -> Result<(), Box<dyn std::error::Error>>
{
    let request = b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\n";
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(request).await?;
    let mut buf = Vec::new();
    let CheckedRead::Request(parsed) =
        read_checked_request_as(&mut server, &mut buf, &guard(), BodyMode::Stream).await?
    else {
        return Err("the request was not handed over".into());
    };
    let mut held = HoldsUntilFlushed::default();
    let sent = Arc::clone(&held.sent);
    continue_if_owed(&mut held, &parsed).await?;
    assert_eq!(&locked(&sent)?[..], b"HTTP/1.1 100 Continue\r\n\r\n");
    Ok(())
}

#[tokio::test]
async fn an_interim_response_is_flushed_to_the_client_before_the_final_one_is_awaited()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut upstream_tx, mut upstream) = duplex(64 * 1024);
    let mut held = HoldsUntilFlushed::default();
    let sent = Arc::clone(&held.sent);
    let relay = tokio::spawn(async move { relay_response(&mut upstream, &mut held, "GET", None, None, false).await });
    upstream_tx
        .write_all(b"HTTP/1.1 103 Early Hints\r\nLink: </a>\r\n\r\n")
        .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while locked(&sent)?.is_empty() {
            tokio::task::yield_now().await;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await??;
    upstream_tx
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .await?;
    relay.await??;
    Ok(())
}

#[tokio::test]
async fn a_bodyless_request_head_is_flushed_upstream_before_the_response_is_awaited()
-> Result<(), Box<dyn std::error::Error>> {
    let request = b"GET /x HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream) = duplex(64 * 1024);
    let proxy = tokio::spawn(async move {
        run_compiled_http_intercept(
            proxy_client,
            UpstreamLeg::connected(HeldWrites::new(proxy_upstream)),
            None,
            &guard(),
        )
        .await
    });
    client.write_all(request).await?;
    expect_bytes(&mut upstream, request, "the request head").await;
    upstream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .await?;
    client.shutdown().await?;
    tokio::time::timeout(WAIT, proxy).await???;
    Ok(())
}

async fn prepared_with(
    mutator: Arc<dyn crate::header_mutator::HeaderMutator>,
    request: &[u8],
) -> Result<(UpstreamRequest, ParsedRequest, String), Box<dyn std::error::Error>> {
    let guard = guard().with_request_checks(Some(
        RequestChecks::new(Arc::new(Fixed(RequestDecision::Allow))).with_header_mutator(mutator),
    ));
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(request).await?;
    let mut buf = Vec::new();
    let parsed = read_checked_request(&mut server, &mut buf, &guard)
        .await?
        .ok_or("expected a value")?;
    let (mut answered, mut sink) = duplex(64 * 1024);
    let prepared = prepare_upstream_request(&mut answered, &parsed, &guard).await?;
    drop(answered);
    let mut answer = String::new();
    sink.read_to_string(&mut answer).await?;
    Ok((prepared, parsed, answer))
}

#[tokio::test]
async fn the_header_mutator_edits_what_goes_upstream_for_the_tls_target() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> = {
        let seen = seen.clone();
        Arc::new(move |headers: &mut Vec<(String, String)>, destination: &Host| {
            if let Ok(mut seen) = seen.lock() {
                seen.push(destination.to_string());
            }
            headers.push(("X-Injected".to_string(), "yes".to_string()));
        })
    };
    let request = b"GET /x HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    let (prepared, parsed, _) = prepared_with(mutator, request).await?;
    let forwarded = String::from_utf8(prepared.resolve(&parsed).raw_bytes().to_vec())?;
    assert!(forwarded.contains("X-Injected: yes\r\n"), "{forwarded}");
    assert_eq!(locked(&seen)?, vec![HOST.to_string()]);
    Ok(())
}

#[tokio::test]
async fn proxy_headers_are_hidden_from_the_mutator_and_dropped_upstream() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> = {
        let seen = seen.clone();
        Arc::new(move |headers: &mut Vec<(String, String)>, _: &Host| {
            if let Ok(mut seen) = seen.lock() {
                seen.extend(headers.iter().cloned());
            }
        })
    };
    let request = b"GET /x HTTP/1.1\r\nHost: api.example.com\r\nProxy-Authorization: Basic dG9rZW4=\r\nProxy-Connection: keep-alive\r\n\r\n";
    let (prepared, parsed, _) = prepared_with(mutator, request).await?;
    let forwarded = String::from_utf8(prepared.resolve(&parsed).raw_bytes().to_vec())?.to_ascii_lowercase();
    assert!(!forwarded.contains("proxy-"), "{forwarded}");
    assert!(
        locked(&seen)?
            .iter()
            .all(|(name, _)| !name.to_ascii_lowercase().starts_with("proxy-"))
    );
    Ok(())
}

#[tokio::test]
async fn hop_by_hop_and_connection_named_headers_are_hidden_from_the_mutator_and_dropped_upstream()
-> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> = {
        let seen = seen.clone();
        Arc::new(move |headers: &mut Vec<(String, String)>, _: &Host| {
            if let Ok(mut seen) = seen.lock() {
                seen.extend(headers.iter().cloned());
            }
        })
    };
    let request = b"POST /x HTTP/1.1\r\nHost: api.example.com\r\nConnection: close, X-Hop, Content-Length\r\nX-Hop: 1\r\nKeep-Alive: timeout=5\r\nTE: trailers\r\nTrailer: X-Sum\r\nUpgrade: websocket\r\nX-Keep: yes\r\nContent-Length: 0\r\n\r\n";
    let (prepared, parsed, _) = prepared_with(mutator, request).await?;
    let forwarded = String::from_utf8(prepared.resolve(&parsed).raw_bytes().to_vec())?.to_ascii_lowercase();
    for gone in ["connection:", "x-hop", "keep-alive", "te:", "trailer:", "upgrade:"] {
        assert!(!forwarded.contains(gone), "{gone} in {forwarded}");
    }
    assert!(forwarded.contains("x-keep: yes"), "{forwarded}");
    assert!(!forwarded.contains("content-length"), "{forwarded}");
    assert!(forwarded.contains("transfer-encoding: chunked"), "{forwarded}");
    assert!(forwarded.contains("host: api.example.com"), "{forwarded}");
    let names: Vec<String> = locked(&seen)?
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"x-keep".to_owned()), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| ["connection", "x-hop", "keep-alive", "te", "trailer", "upgrade"].contains(&n.as_str())),
        "{names:?}"
    );
    Ok(())
}

async fn forwarded_after_strip(request: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> = Arc::new(|_: &mut Vec<(String, String)>, _: &Host| {});
    let (prepared, parsed, _) = prepared_with(mutator, request).await?;
    Ok(String::from_utf8(prepared.resolve(&parsed).raw_bytes().to_vec())?)
}

#[tokio::test]
async fn hop_by_hop_names_match_in_any_spelling_of_the_separators() -> Result<(), Box<dyn std::error::Error>> {
    let forwarded = forwarded_after_strip(
        b"GET /x HTTP/1.1\r\nHost: api.example.com\r\nConnection: X_Hop, x.other\r\nX.Hop: 1\r\nX-Other: 2\r\nKeep_Alive: timeout=5\r\nProxy.Authorization: Basic dG9rZW4=\r\nX-Keep: yes\r\n\r\n",
    )
    .await?
    .to_ascii_lowercase();
    for gone in ["x.hop", "x-other", "keep_alive", "proxy.authorization", "connection:"] {
        assert!(!forwarded.contains(gone), "{gone} in {forwarded}");
    }
    assert!(forwarded.contains("x-keep: yes"), "{forwarded}");
    Ok(())
}

#[tokio::test]
async fn a_host_that_connection_names_is_stripped_and_rebuilt_from_the_target() -> Result<(), Box<dyn std::error::Error>>
{
    let forwarded =
        forwarded_after_strip(b"GET /x HTTP/1.1\r\nHost: api.example.com\r\nConnection: Host\r\n\r\n").await?;
    let lowered = forwarded.to_ascii_lowercase();
    assert_eq!(lowered.matches("host:").count(), 1, "{forwarded}");
    assert!(forwarded.contains("Host: api.example.com\r\n"), "{forwarded}");
    assert!(!lowered.contains("connection:"), "{forwarded}");
    Ok(())
}

#[tokio::test]
async fn the_host_is_the_targets_authority_whatever_spelling_the_client_used() -> Result<(), Box<dyn std::error::Error>>
{
    let forwarded = forwarded_after_strip(b"GET /x HTTP/1.1\r\nHost: API.Example.com:443\r\n\r\n").await?;
    assert!(forwarded.contains("Host: api.example.com\r\n"), "{forwarded}");
    assert!(!forwarded.contains("API"), "{forwarded}");
    Ok(())
}

#[tokio::test]
async fn a_content_length_that_connection_names_is_stripped_and_the_body_is_chunked()
-> Result<(), Box<dyn std::error::Error>> {
    let forwarded = forwarded_after_strip(
        b"POST /x HTTP/1.1\r\nHost: api.example.com\r\nConnection: Content-Length\r\nContent-Length: 5\r\n\r\nhello",
    )
    .await?;
    let lowered = forwarded.to_ascii_lowercase();
    assert!(!lowered.contains("content-length"), "{forwarded}");
    assert!(lowered.contains("transfer-encoding: chunked\r\n"), "{forwarded}");
    assert!(forwarded.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"), "{forwarded}");
    Ok(())
}

#[tokio::test]
async fn a_transfer_encoding_that_connection_names_is_stripped_and_the_body_stays_chunked()
-> Result<(), Box<dyn std::error::Error>> {
    let forwarded = forwarded_after_strip(
        b"POST /x HTTP/1.1\r\nHost: api.example.com\r\nConnection: Transfer-Encoding\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    )
    .await?;
    let lowered = forwarded.to_ascii_lowercase();
    assert_eq!(lowered.matches("transfer-encoding:").count(), 1, "{forwarded}");
    assert!(!lowered.contains("connection:"), "{forwarded}");
    assert!(forwarded.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"), "{forwarded}");
    Ok(())
}

#[tokio::test]
async fn a_streamed_body_is_chunked_as_it_is_relayed_once_its_content_length_is_stripped()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(b"hello").await?;
    let (mut upstream, mut sink) = duplex(64 * 1024);
    let mut buf = Vec::new();
    let sent = relay_rechunked_body(
        &mut server,
        &mut buf,
        &mut upstream,
        Framing::ContentLength(5),
        Framing::Chunked,
    )
    .await?;
    drop(upstream);
    let mut out = String::new();
    sink.read_to_string(&mut out).await?;
    assert_eq!(out, "5\r\nhello\r\n0\r\n\r\n");
    assert_eq!(sent, out.len() as u64);
    Ok(())
}

#[tokio::test]
async fn proxy_headers_are_dropped_upstream_without_a_mutator() -> Result<(), Box<dyn std::error::Error>> {
    let guard = guard().with_request_checks(Some(RequestChecks::new(Arc::new(Fixed(RequestDecision::Allow)))));
    let (mut client, mut server) = duplex(64 * 1024);
    client
        .write_all(b"GET /x HTTP/1.1\r\nHost: api.example.com\r\nProxy-Authorization: Basic dG9rZW4=\r\n\r\n")
        .await?;
    let mut buf = Vec::new();
    let parsed = read_checked_request(&mut server, &mut buf, &guard)
        .await?
        .ok_or("expected a value")?;
    let (mut answered, _sink) = duplex(64 * 1024);
    let prepared = prepare_upstream_request(&mut answered, &parsed, &guard).await?;
    let forwarded = String::from_utf8(prepared.resolve(&parsed).raw_bytes().to_vec())?.to_ascii_lowercase();
    assert!(!forwarded.contains("proxy-authorization"), "{forwarded}");
    Ok(())
}

#[tokio::test]
async fn a_header_mutator_that_breaks_the_framing_gets_the_request_denied() -> Result<(), Box<dyn std::error::Error>> {
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> =
        Arc::new(|headers: &mut Vec<(String, String)>, _: &Host| {
            headers.retain(|(name, _)| !name.eq_ignore_ascii_case("host"));
        });
    let request = b"GET /x HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    let (prepared, _, answer) = prepared_with(mutator, request).await?;
    assert!(matches!(prepared, UpstreamRequest::Refused));
    assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    Ok(())
}

#[tokio::test]
async fn an_upstream_100_is_dropped_once_the_proxy_has_told_the_client_to_continue()
-> Result<(), Box<dyn std::error::Error>> {
    let response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    for (expects, interim) in [(true, false), (false, true)] {
        let (mut upstream_tx, mut upstream) = tokio::io::duplex(4096);
        upstream_tx.write_all(response).await?;
        let mut client = Vec::new();
        relay_response(&mut upstream, &mut client, "POST", None, None, expects).await?;
        let client = String::from_utf8(client)?;
        assert_eq!(client.contains("100 Continue"), interim, "{client}");
        assert!(client.contains("200 OK"));
    }
    Ok(())
}

#[tokio::test]
async fn a_relayed_response_has_reached_the_client_when_it_counts_as_sent() -> Result<(), Box<dyn std::error::Error>> {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
    let (mut upstream_tx, mut upstream) = duplex(4096);
    upstream_tx.write_all(response).await?;
    let mut sent = Vec::new();
    let mut client = tokio::io::BufWriter::new(&mut sent);
    let relayed = relay_response_or_bad_gateway(
        &mut upstream,
        &mut client,
        "GET",
        None,
        None,
        false,
        HopByHop::Relayed,
        StreamUse::Fresh,
    )
    .await?;
    assert_eq!(relayed.connection, Connection::Close);
    assert!(client.buffer().is_empty());
    drop(client);
    assert_eq!(sent, response);
    Ok(())
}

#[tokio::test]
async fn the_response_hook_sees_the_status_and_headers() -> Result<(), Box<dyn std::error::Error>> {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let hook = {
        let seen = seen.clone();
        ResponseHook::new(move |status, headers| {
            if let Ok(mut seen) = seen.lock() {
                *seen = Some((status, headers.to_vec()));
            }
        })
    };
    let (mut upstream_tx, mut upstream) = duplex(4096);
    upstream_tx
        .write_all(b"HTTP/1.1 201 Created\r\nX-Id: 7\r\nContent-Length: 0\r\n\r\n")
        .await?;
    let mut client = Vec::new();
    relay_response(&mut upstream, &mut client, "POST", None, Some(&hook), false).await?;
    let (status, headers) = locked(&seen)?.ok_or("expected a value")?;
    assert_eq!(status, 201);
    assert!(headers.contains(&("X-Id".to_string(), "7".to_string())));
    Ok(())
}

fn rules(match_rules: Vec<MatchRule>) -> Result<CompiledMatchRules, Box<dyn std::error::Error>> {
    Ok(CompiledMatchRules::compile(&match_rules)?)
}

fn path_rule(matcher: StringMatcher) -> MatchRule {
    MatchRule {
        path: Some(matcher),
        methods: Vec::new(),
        query_string: Vec::new(),
        headers: Vec::new(),
    }
}

fn get(target: &str) -> Vec<u8> {
    format!("GET {target} HTTP/1.1\r\nHost: {HOST}\r\n\r\n").into_bytes()
}

async fn parse_with(guard: &RequestGuard, bytes: &[u8]) -> Result<Option<ParsedRequest>, HttpInterceptError> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(bytes).await.map_err(HttpInterceptError::ClientWrite)?;
    client.shutdown().await.map_err(HttpInterceptError::ClientWrite)?;
    let mut buf = Vec::new();
    read_request(&mut server, &mut buf, guard).await
}

async fn parse(bytes: &[u8]) -> Result<Option<ParsedRequest>, HttpInterceptError> {
    parse_with(&fixed_guard(RequestDecision::Allow), bytes).await
}

fn refused(
    outcome: Result<Option<ParsedRequest>, HttpInterceptError>,
) -> Result<RefusalError, Box<dyn std::error::Error>> {
    match outcome {
        Err(HttpInterceptError::Refused(refusal)) => Ok(refusal),
        Ok(_) => Err("request was accepted".into()),
        Err(other) => Err(format!("unexpected error: {other}").into()),
    }
}

async fn refusal_of(bytes: &[u8]) -> Result<RefusalError, Box<dyn std::error::Error>> {
    refused(parse(bytes).await)
}

fn request_line_target(request: &ParsedRequest) -> Result<String, Box<dyn std::error::Error>> {
    let line_end = request
        .raw
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or("expected a value")?;
    let line = std::str::from_utf8(&request.raw[..line_end])?;
    Ok(line.split(' ').nth(1).ok_or("expected a value")?.to_string())
}

async fn forwarded_target(target: &str) -> Result<String, Box<dyn std::error::Error>> {
    let request = parse(&get(target)).await?.ok_or("expected a value")?;
    request_line_target(&request)
}

#[tokio::test]
async fn empty_rule_set_matches_every_request() -> Result<(), Box<dyn std::error::Error>> {
    let request = parse(&get("/anything")).await?.ok_or("expected a value")?;
    assert!(rules(Vec::new())?.matches(&request));
    Ok(())
}

#[test]
fn invalid_regex_is_rejected_when_rules_are_compiled() -> Result<(), Box<dyn std::error::Error>> {
    let err = CompiledMatchRules::compile(&[path_rule(StringMatcher::Regex("(".to_string()))])
        .err()
        .ok_or("expected a value")?;
    assert_eq!(err.pattern, "(");
    Ok(())
}

#[tokio::test]
async fn compiled_regex_matches_without_recompiling() -> Result<(), Box<dyn std::error::Error>> {
    let compiled = rules(vec![path_rule(StringMatcher::Regex("^/v[0-9]+/".to_string()))])?;
    let hit = parse(&get("/v2/users")).await?.ok_or("expected a value")?;
    let miss = parse(&get("/health")).await?.ok_or("expected a value")?;
    assert!(compiled.matches(&hit));
    assert!(!compiled.matches(&miss));
    Ok(())
}

#[tokio::test]
async fn trace_is_refused_with_405_and_other_spellings_as_methods_server_does_not_parse()
-> Result<(), Box<dyn std::error::Error>> {
    let request = format!("TRACE / HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    let refusal = refusal_of(request.as_bytes()).await?;
    assert_eq!(refusal, RefusalError::EchoMethod);
    assert_eq!(refusal.status(), StatusCode::METHOD_NOT_ALLOWED);
    for method in ["trace", "TRACK", "Track"] {
        let request = format!("{method} / HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
        let refusal = refusal_of(request.as_bytes()).await?;
        assert_eq!(refusal, RefusalError::Unparsed(HeadParseError::InvalidMethod));
        assert_eq!(refusal.status(), StatusCode::BAD_REQUEST);
    }
    Ok(())
}

const AMBIGUOUS: &[&str] = &[
    "/a/../b",
    "/a/./b",
    "/a/..",
    "/a/.",
    "/a/%2e%2e/b",
    "/a/.%2E/b",
    "/a/%2e/b",
    "/a;/../b",
    "/a//../b",
    "//k/x",
    "///k?q=1",
    "/a\\b",
    "/\\k",
    "//k/../b",
    "//k\\b",
    "/\\/k",
    "/a\\..\\b",
    "/a\\%2e%2e\\b",
    "/a\\b/../c",
    "/a%5cb/../c",
    "//k%5cb",
    "/a%23/../b",
    "/a%23b\\c",
    "//k%23",
    "http://api.example.com/a/../b",
];

const UNAMBIGUOUS: &[&str] = &[
    "/",
    "/a/b/?x=1",
    "/a//b",
    "/a/b//",
    "/a%2Fb",
    "/a%2f..%2fb",
    "/a/..%2fb",
    "/a/%252e%252e/b",
    "/a/...",
    "/a/..b",
    "/a/.hidden",
    "/a/..;/b",
    "/a;p=1/b;q?x=1",
    "/a?x=../y&z=//&w=\\",
    "/%61%7e",
    "/a%zz",
    "/a%5cb",
    "/a%5c..%5cb",
    "/a/..%5cb",
    "/a%23b",
    "/a/..%23",
    "/a/%2e%2e%23",
    "/a?x=%23",
    "/a?",
    "/a??",
    "/a{b}|c^d\"e<f>g`h",
    "/a?q='x'&r=\"y\"",
];

#[tokio::test]
async fn ambiguous_request_targets_are_refused_with_400() -> Result<(), Box<dyn std::error::Error>> {
    for target in AMBIGUOUS {
        let refusal = refusal_of(&get(target)).await?;
        assert_eq!(refusal, RefusalError::AmbiguousTarget, "{target}");
        assert_eq!(refusal.status(), StatusCode::BAD_REQUEST, "{target}");
    }
    Ok(())
}

fn allowing_guard(mode: AmbiguousTargets) -> RequestGuard {
    fixed_guard(RequestDecision::Allow).with_ambiguous_targets(mode)
}

async fn forwarded_target_in(mode: AmbiguousTargets, target: &str) -> Result<String, Box<dyn std::error::Error>> {
    let request = parse_with(&allowing_guard(mode), &get(target))
        .await?
        .ok_or("expected a value")?;
    let target = request_line_target(&request)?;
    Ok(target
        .strip_prefix(&format!("http://{HOST}"))
        .unwrap_or(&target)
        .to_string())
}

#[tokio::test]
async fn as_spelled_forwards_an_ambiguous_target_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    for target in AMBIGUOUS {
        assert_eq!(
            &forwarded_target_in(AmbiguousTargets::AsSpelled, target).await?,
            target.strip_prefix(&format!("http://{HOST}")).unwrap_or(target),
            "{target}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn normalize_forwards_the_normalized_form_of_an_ambiguous_target() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(forwarded_target_in(AmbiguousTargets::Normalize, "/a/../b").await?, "/b");
    assert_eq!(
        forwarded_target_in(AmbiguousTargets::Normalize, "/a/./b").await?,
        "/a/b"
    );
    Ok(())
}

#[tokio::test]
async fn without_request_checks_an_ambiguous_target_is_forwarded_as_spelled() -> Result<(), Box<dyn std::error::Error>>
{
    for target in AMBIGUOUS {
        let request = parse_with(&guard(), &get(target)).await?.ok_or("expected a value")?;
        let line_end = request
            .raw
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("expected a value")?;
        let line = std::str::from_utf8(&request.raw[..line_end])?;
        let forwarded = line.split(' ').nth(1).ok_or("expected a value")?;
        assert_eq!(
            forwarded,
            target.strip_prefix(&format!("http://{HOST}")).unwrap_or(target),
            "{target}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn unambiguous_request_targets_are_forwarded_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    for target in UNAMBIGUOUS {
        assert_eq!(&forwarded_target(target).await?, target, "{target}");
    }
    Ok(())
}

#[tokio::test]
async fn fragments_are_cut_before_judging_and_forwarding() -> Result<(), Box<dyn std::error::Error>> {
    for (sent, left) in [
        ("/p#f", "/p"),
        ("/p#", "/p"),
        ("/p#f?q", "/p"),
        ("/p?q#f", "/p?q"),
        ("/p?#f", "/p?"),
        ("/#f", "/"),
        ("/p#f#g", "/p"),
        ("/p%23a#f", "/p%23a"),
        ("/p#/../x", "/p"),
        ("/p#\\x", "/p"),
    ] {
        assert_eq!(forwarded_target(sent).await?, left, "{sent}");
    }
    Ok(())
}

#[tokio::test]
async fn ambiguity_survives_a_fragment() -> Result<(), Box<dyn std::error::Error>> {
    for sent in ["/a/../b#x", "/a/%2e%2e/b#x", "//k#x", "/a\\b#x"] {
        assert_eq!(refusal_of(&get(sent)).await?, RefusalError::AmbiguousTarget, "{sent}");
    }
    Ok(())
}

#[tokio::test]
async fn absolute_form_target_is_forwarded_as_origin_form() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(forwarded_target("http://api.example.com/a?x=1").await?, "/a?x=1");
    assert_eq!(forwarded_target("https://api.example.com").await?, "/");
    Ok(())
}

#[tokio::test]
async fn non_ascii_targets_are_forwarded_percent_encoded() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        forwarded_target("/caf\u{e9}?q=\u{20ac}").await?,
        "/caf%C3%A9?q=%E2%82%AC"
    );
    Ok(())
}

#[tokio::test]
async fn malformed_targets_are_refused_with_400() -> Result<(), Box<dyn std::error::Error>> {
    for target in ["foo", "ftp://api.example.com/a", "http://", "/a b"] {
        let request = format!("GET {target} HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
        let refusal = refusal_of(request.as_bytes()).await?;
        assert_eq!(refusal.status(), StatusCode::BAD_REQUEST, "{target}");
    }
    let replacement_char = refusal_of(&get("/a\u{fffd}")).await?;
    assert_eq!(replacement_char, RefusalError::TargetBytes);
    Ok(())
}

#[tokio::test]
async fn asterisk_target_is_only_valid_for_options() -> Result<(), Box<dyn std::error::Error>> {
    let options = b"OPTIONS * HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    assert!(parse(options).await?.is_some());
    let get_star = b"GET * HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    assert_eq!(refusal_of(get_star).await?, RefusalError::MalformedTarget);
    Ok(())
}

#[tokio::test]
async fn host_must_match_the_tls_target() -> Result<(), Box<dyn std::error::Error>> {
    let cases: &[(&str, bool)] = &[
        ("api.example.com", true),
        ("API.Example.COM", true),
        ("api.example.com:443", true),
        ("api.example.com:8443", false),
        ("evil.example.com", false),
        ("api.example.com.", false),
        ("api.example.com:", false),
        ("", false),
        ("[api.example.com]", false),
        ("api.example.com@evil.example.com", false),
    ];
    for (host, accepted) in cases {
        let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n");
        let outcome = parse(request.as_bytes()).await;
        if *accepted {
            assert!(outcome?.is_some(), "{host}");
        } else {
            let refusal = refused(outcome)?;
            assert!(
                matches!(refusal, RefusalError::HostMismatch(_) | RefusalError::MissingHost),
                "{host}: {refusal:?}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn host_mismatch_is_421_and_missing_host_is_400() -> Result<(), Box<dyn std::error::Error>> {
    let mismatch = b"GET / HTTP/1.1\r\nHost: evil.example.com\r\n\r\n";
    let refusal = refusal_of(mismatch).await?;
    assert_eq!(refusal, RefusalError::HostMismatch(HostMismatchError::Header));
    assert_eq!(refusal.status(), StatusCode::MISDIRECTED_REQUEST);

    let missing = refusal_of(b"GET / HTTP/1.1\r\n\r\n").await?;
    assert_eq!(missing, RefusalError::MissingHost);
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn a_host_that_does_not_match_is_forwarded_when_matching_is_off() -> Result<(), Box<dyn std::error::Error>> {
    let guard = RequestGuard::new(TlsTarget::new(HOST, 443)).with_host_match(HostMatch::NotRequired);
    for request in [&b"GET / HTTP/1.1\r\nHost: evil.example.com\r\n\r\n"[..]] {
        assert!(parse_with(&guard, request).await?.is_some());
    }
    Ok(())
}

#[tokio::test]
async fn a_request_without_a_host_is_400_whether_or_not_matching_is_on() -> Result<(), Box<dyn std::error::Error>> {
    for host_match in [HostMatch::Required, HostMatch::NotRequired] {
        let guard = RequestGuard::new(TlsTarget::new(HOST, 443)).with_host_match(host_match);
        let refusal = refused(parse_with(&guard, b"GET / HTTP/1.1\r\n\r\n").await)?;
        assert_eq!(refusal, RefusalError::MissingHost);
        assert_eq!(refusal.status(), StatusCode::BAD_REQUEST);
    }
    Ok(())
}

#[tokio::test]
async fn the_tls_server_name_must_name_the_target_only_when_matching_is_on() -> Result<(), Box<dyn std::error::Error>> {
    let request = b"GET / HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    let guard = |host_match, name| {
        RequestGuard::new(TlsTarget::new(HOST, 443))
            .with_host_match(host_match)
            .with_client_server_name(name)
    };
    let other = || ClientServerName::Name("other.example.com".to_string());
    for (host_match, name, refusal) in [
        (HostMatch::Required, other(), Some(HostMismatchError::ServerName)),
        (
            HostMatch::Required,
            ClientServerName::Unreadable,
            Some(HostMismatchError::ServerNameUnreadable),
        ),
        (
            HostMatch::Required,
            ClientServerName::Name("API.example.com".to_string()),
            None,
        ),
        (HostMatch::Required, ClientServerName::Absent, None),
        (HostMatch::NotRequired, other(), None),
        (HostMatch::NotRequired, ClientServerName::Unreadable, None),
    ] {
        let outcome = parse_with(&guard(host_match, name.clone()), request).await;
        match refusal {
            Some(mismatch) => {
                let got = refused(outcome)?;
                assert_eq!(got, RefusalError::HostMismatch(mismatch), "{name:?}");
                assert_eq!(got.status(), StatusCode::MISDIRECTED_REQUEST);
            }
            None => assert!(outcome?.is_some(), "{host_match:?} {name:?}"),
        }
    }
    Ok(())
}

#[tokio::test]
async fn host_port_must_match_a_non_default_tls_port() -> Result<(), Box<dyn std::error::Error>> {
    let guard = RequestGuard::new(TlsTarget::new(HOST, 8443));
    let ok = b"GET / HTTP/1.1\r\nHost: api.example.com:8443\r\n\r\n";
    assert!(parse_with(&guard, ok).await?.is_some());
    let default_port = b"GET / HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    assert!(parse_with(&guard, default_port).await.is_err());
    Ok(())
}

#[tokio::test]
async fn ipv6_host_header_requires_brackets() -> Result<(), Box<dyn std::error::Error>> {
    let guard = RequestGuard::new(TlsTarget::new("::1", 443));
    let bracketed = b"GET / HTTP/1.1\r\nHost: [::1]\r\n\r\n";
    assert!(parse_with(&guard, bracketed).await?.is_some());
    let bare = b"GET / HTTP/1.1\r\nHost: ::1\r\n\r\n";
    assert!(parse_with(&guard, bare).await.is_err());
    Ok(())
}

#[tokio::test]
async fn repeated_host_headers_are_421() -> Result<(), Box<dyn std::error::Error>> {
    let request = b"GET / HTTP/1.1\r\nHost: api.example.com\r\nHost: api.example.com\r\n\r\n";
    let refusal = refusal_of(request).await?;
    assert_eq!(refusal, RefusalError::HostMismatch(HostMismatchError::MultipleHeaders));
    Ok(())
}

#[tokio::test]
async fn absolute_form_authority_must_name_the_tls_target() -> Result<(), Box<dyn std::error::Error>> {
    let request = b"GET http://evil.example.com/ HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    let refusal = refusal_of(request).await?;
    assert_eq!(refusal, RefusalError::HostMismatch(HostMismatchError::Authority));
    Ok(())
}

#[tokio::test]
async fn content_length_and_transfer_encoding_together_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    for (fields, error) in [
        (
            "Content-Length: 4\r\nTransfer-Encoding: chunked",
            HeadParseError::TransferEncodingAfterContentLength,
        ),
        (
            "Transfer-Encoding: chunked\r\nContent-Length: 4",
            HeadParseError::ContentLengthAfterTransferEncoding,
        ),
        (
            "Transfer-Encoding: gzip\r\nContent-Length: 4",
            HeadParseError::ContentLengthAfterTransferEncoding,
        ),
    ] {
        let request = format!("POST / HTTP/1.1\r\nHost: {HOST}\r\n{fields}\r\n\r\n0\r\n\r\n");
        assert_eq!(
            refusal_of(request.as_bytes()).await?,
            RefusalError::Unparsed(error),
            "{fields}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn conflicting_content_lengths_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let request = b"POST / HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab";
    assert_eq!(
        refusal_of(request).await?,
        RefusalError::Unparsed(HeadParseError::DuplicateContentLength)
    );
    Ok(())
}

#[tokio::test]
async fn header_name_with_space_before_colon_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let request = b"POST / HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding : chunked\r\n\r\n";
    assert_eq!(
        refusal_of(request).await?,
        RefusalError::Unparsed(HeadParseError::InvalidHeaderToken)
    );
    Ok(())
}

#[tokio::test]
async fn chunked_request_body_is_parsed_and_forwarded_with_framing() -> Result<(), Box<dyn std::error::Error>> {
    let request = b"POST /upload HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n";
    let parsed = parse(request).await?.ok_or("expected a value")?;
    let canonical = b"POST /upload HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    assert_eq!(parsed.raw_bytes(), &canonical[..]);
    Ok(())
}

#[tokio::test]
async fn oversized_chunked_body_is_refused_with_413() -> Result<(), Box<dyn std::error::Error>> {
    let small = RequestGuard::new(TlsTarget::new(HOST, 443)).with_body_limit(BodyLimit::new(8));
    let request = b"POST / HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
    let refusal = refused(parse_with(&small, request).await)?;
    assert_eq!(refusal, RefusalError::BodyTooLarge);
    assert_eq!(refusal.status(), StatusCode::PAYLOAD_TOO_LARGE);
    Ok(())
}

#[tokio::test]
async fn oversized_content_length_is_refused_before_reading_the_body() -> Result<(), Box<dyn std::error::Error>> {
    let small = RequestGuard::new(TlsTarget::new(HOST, 443)).with_body_limit(BodyLimit::new(8));
    let request = b"POST / HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 9\r\n\r\n";
    assert!(matches!(
        parse_with(&small, request).await,
        Err(HttpInterceptError::Refused(RefusalError::BodyTooLarge))
    ));
    Ok(())
}

#[tokio::test]
async fn malformed_chunk_framing_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    for (body, error) in [
        ("zz\r\nhello\r\n0\r\n\r\n", HeadParseError::InvalidChunkSize),
        ("5\r\nhelloXX0\r\n\r\n", HeadParseError::ExpectedLfAfterChunkData),
    ] {
        let request = format!("POST / HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n{body}");
        assert_eq!(refusal_of(request.as_bytes()).await?, RefusalError::UnparsedBody(error));
    }
    Ok(())
}

#[tokio::test]
async fn request_after_chunked_request_with_trailers_has_its_own_head() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client
        .write_all(
            b"POST /first HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nX-Late: 1\r\n\r\nGET /second HTTP/1.1\r\nHost: api.example.com\r\n\r\n",
        )
        .await
        ?;
    client.shutdown().await?;

    let guard = guard();
    let mut buf = Vec::new();
    let first = read_request(&mut server, &mut buf, &guard)
        .await?
        .ok_or("expected a value")?;
    let second = read_request(&mut server, &mut buf, &guard)
        .await?
        .ok_or("expected a value")?;
    assert_eq!(first.path, "/first");
    assert_eq!(second.path, "/second");
    assert!(second.headers.iter().all(|(name, _)| name != "X-Late"));
    assert!(read_request(&mut server, &mut buf, &guard).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn refused_request_is_answered_with_status_tag_and_reason() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client
        .write_all(b"TRACE / HTTP/1.1\r\nHost: api.example.com\r\n\r\n")
        .await?;
    let mut buf = Vec::new();
    let outcome = read_checked_request(&mut server, &mut buf, &guard()).await?;
    assert!(outcome.is_none());
    drop(server);

    let mut response = String::new();
    client.read_to_string(&mut response).await?;
    assert_eq!(
        crate::framed_head::undated(&response),
        "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n22\r\nthe TRACE method is not forwarded\n\r\n0\r\n\r\n"
    );
    Ok(())
}

const ANSWERED_AT: std::time::Duration = std::time::Duration::from_secs(1_700_000_000);
const ANSWER_DATE: &str = "Date: Tue, 14 Nov 2023 22:13:20 GMT\r\n";

fn answer_text(refusal: RefusalError, request: AnsweredRequest) -> Result<String, Box<dyn std::error::Error>> {
    Ok(String::from_utf8(
        refusal.answer(request, std::time::UNIX_EPOCH + ANSWERED_AT),
    )?)
}

#[test]
fn a_misdirected_request_is_answered_with_its_status_tag_and_reason() -> Result<(), Box<dyn std::error::Error>> {
    let refusal = RefusalError::HostMismatch(HostMismatchError::Header);
    assert_eq!(
        answer_text(refusal, AnsweredRequest::new("GET", false))?,
        format!(
            "HTTP/1.1 421 Misdirected Request\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\n{ANSWER_DATE}Connection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\n2e\r\nHost header does not match the request target\n\r\n0\r\n\r\n"
        )
    );
    Ok(())
}

#[test]
fn an_ambiguous_target_is_answered_chunked_and_a_head_without_body() -> Result<(), Box<dyn std::error::Error>> {
    let body = format!("{}\n", RefusalError::AmbiguousTarget);
    assert_eq!(
        answer_text(RefusalError::AmbiguousTarget, AnsweredRequest::new("GET", false))?,
        format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nX-Proxy-Error: ambiguous-request-target\r\n{ANSWER_DATE}Connection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\nd8\r\n{body}\r\n0\r\n\r\n"
        )
    );
    assert_eq!(
        answer_text(RefusalError::AmbiguousTarget, AnsweredRequest::new("HEAD", true))?,
        format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nX-Proxy-Error: ambiguous-request-target\r\n{ANSWER_DATE}Connection: close\r\n\r\n"
        )
    );
    Ok(())
}

#[test]
fn a_missing_host_is_answered_as_framed_answers_it() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        answer_text(RefusalError::MissingHost, AnsweredRequest::new("GET", false))?,
        format!(
            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n{ANSWER_DATE}Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
        )
    );
    Ok(())
}

#[test]
fn a_head_the_parser_refused_is_answered_on_the_raw_socket() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        answer_text(
            RefusalError::Unparsed(HeadParseError::InvalidHeaderToken),
            AnsweredRequest::new("GET", false)
        )?,
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\nContent-Length: 54\r\nConnection: close\r\n\r\nmalformed request (Parse Error: Invalid header token)\n"
    );
    Ok(())
}

#[test]
fn a_refusal_to_an_http_1_0_request_is_written_whole_and_closes() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        answer_text(
            RefusalError::RequestDenied,
            AnsweredRequest::of_head(b"GET / HTTP/1.0\r\nHost: a\r\n\r\n")
        )?,
        format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nX-Proxy-Error: blocked-by-egress-proxy\r\n{ANSWER_DATE}Connection: close\r\n\r\ndenied by sandbox policy\n"
        )
    );
    Ok(())
}

fn test_transform(match_rules: Vec<MatchRule>) -> Result<CredentialTransform, Box<dyn std::error::Error>> {
    Ok(CredentialTransform {
        match_rules,
        credential: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("test-secret"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
    })
}

#[tokio::test]
async fn refused_request_never_reaches_upstream() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client_side, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(64 * 1024);
    let transform = test_transform(Vec::new())?;
    let source = FakeCredentialSource::new().with_token("test-secret", "tok");

    client_side
        .write_all(b"TRACE / HTTP/1.1\r\nHost: api.example.com\r\n\r\n")
        .await?;
    let guard = guard();
    let counts = run_http_intercept(proxy_client, proxy_upstream, &transform, &source, &guard).await?;
    assert_eq!(counts.client_to_upstream, 0);

    let mut forwarded = Vec::new();
    upstream_side.read_to_end(&mut forwarded).await?;
    assert!(forwarded.is_empty());

    let mut response = String::new();
    client_side.read_to_string(&mut response).await?;
    assert!(response.starts_with("HTTP/1.1 405"));
    Ok(())
}

#[tokio::test]
async fn chunked_request_gets_credential_injected_and_body_preserved() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client_side, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream_side) = duplex(64 * 1024);
    let transform = test_transform(Vec::new())?;
    let source = FakeCredentialSource::new().with_token("test-secret", "tok");

    let upstream = tokio::spawn(async move {
        let mut received = Vec::new();
        let mut chunk = [0u8; 4096];
        while !received.ends_with(b"0\r\n\r\n") {
            let n = upstream_side.read(&mut chunk).await?;
            received.extend_from_slice(&chunk[..n]);
        }
        upstream_side
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await?;
        Ok::<_, std::io::Error>(received)
    });

    client_side
        .write_all(
            b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        )
        .await?;
    client_side.shutdown().await?;

    let guard = guard();
    run_http_intercept(proxy_client, proxy_upstream, &transform, &source, &guard).await?;

    let received = String::from_utf8(upstream.await??)?;
    assert!(received.contains("authorization: tok\r\n"));
    assert!(received.contains("Transfer-Encoding: chunked\r\n"));
    assert!(received.ends_with("\r\n\r\n5\r\nhello\r\n0\r\n\r\n"));
    Ok(())
}

#[tokio::test]
async fn invalid_regex_in_transform_fails_before_any_request_is_handled() -> Result<(), Box<dyn std::error::Error>> {
    let (_client_side, proxy_client) = duplex(1024);
    let (proxy_upstream, _upstream_side) = duplex(1024);
    let transform = test_transform(vec![path_rule(StringMatcher::Regex("(".to_string()))])?;
    let source = FakeCredentialSource::new();
    let guard = guard();
    let outcome = run_http_intercept(proxy_client, proxy_upstream, &transform, &source, &guard).await;
    assert!(matches!(outcome, Err(HttpInterceptError::InvalidMatchRule(_))));
    Ok(())
}
