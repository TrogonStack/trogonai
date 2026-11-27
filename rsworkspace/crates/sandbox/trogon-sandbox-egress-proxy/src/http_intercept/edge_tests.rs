use super::*;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use tokio::io::{DuplexStream, ReadBuf, duplex};
use trogon_sandbox_contract::{DomainPattern, HeaderName, InjectHosts, ShortLivedToken};
use trogon_sandbox_protocol::{RealValue, SentinelRegistry};

use crate::held_writes::WAIT;
use crate::request_decision::HeaderEdits;
use crate::request_filter::RequestFilter;
use crate::secret_substitution::SecretSubstitution;
use crate::secret_substitution::tests::{fixture, scope};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

const HOST: &str = "api.example.com";

fn guard() -> RequestGuard {
    RequestGuard::new(TlsTarget::new(HOST, 443))
}

fn logging() -> tracing::subscriber::DefaultGuard {
    tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(std::io::sink)
            .finish(),
    )
}

async fn parsed(bytes: &[u8]) -> TestResult<ParsedRequest> {
    parsed_under(&guard(), bytes).await
}

async fn parsed_under(guard: &RequestGuard, bytes: &[u8]) -> TestResult<ParsedRequest> {
    let (mut client, mut server) = duplex(128 * 1024);
    client.write_all(bytes).await?;
    client.shutdown().await?;
    let mut buf = Vec::new();
    Ok(read_request(&mut server, &mut buf, guard)
        .await?
        .ok_or("no request was read")?)
}

fn get(target: &str) -> Vec<u8> {
    format!("GET {target} HTTP/1.1\r\nHost: {HOST}\r\n\r\n").into_bytes()
}

struct Fixed(RequestDecision);

#[async_trait::async_trait]
impl RequestFilter for Fixed {
    async fn decide(&self, _request: &RequestView<'_>) -> RequestDecision {
        self.0.clone()
    }
}

struct ReadsBody;

#[async_trait::async_trait]
impl RequestFilter for ReadsBody {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        if let Some(body) = &request.body {
            let _ = body.read(8).await;
        }
        RequestDecision::Allow
    }
}

fn checked_guard(filter: impl RequestFilter + 'static) -> RequestGuard {
    guard().with_request_checks(Some(RequestChecks::new(Arc::new(filter))))
}

fn query(key: &str, value: StringMatcher) -> QueryMatcher {
    QueryMatcher {
        key: key.to_string(),
        value,
    }
}

fn header(name: &str, value: StringMatcher) -> TestResult<HeaderMatcher> {
    Ok(HeaderMatcher {
        name: HeaderName::new(name)?,
        value,
    })
}

fn rule(methods: Vec<HttpMethod>, query_string: Vec<QueryMatcher>, headers: Vec<HeaderMatcher>) -> MatchRule {
    MatchRule {
        path: None,
        methods,
        query_string,
        headers,
    }
}

fn matching(rule: MatchRule, request: &ParsedRequest) -> TestResult<bool> {
    Ok(CompiledMatchRules::compile(&[rule])?.matches(request))
}

fn exact(value: &str) -> StringMatcher {
    StringMatcher::Exact(value.to_string())
}

#[tokio::test]
async fn query_and_header_rules_match_what_the_request_carries() -> TestResult<()> {
    let request = parsed(b"GET /x?a=1&team=red HTTP/1.1\r\nHost: api.example.com\r\nX-Env: prod-eu\r\n\r\n").await?;
    let prefix = StringMatcher::StartsWith("prod".to_string());
    assert!(matching(
        rule(
            Vec::new(),
            vec![query("team", exact("red"))],
            vec![header("x-env", prefix)?]
        ),
        &request
    )?);
    let pattern = |text: &str| StringMatcher::Regex(text.to_string());
    assert!(matching(
        rule(
            vec![HttpMethod::new("GET")?],
            vec![query("team", pattern("^r.d$"))],
            vec![header("X-ENV", pattern("eu$"))?]
        ),
        &request
    )?);
    assert!(!matching(
        rule(vec![HttpMethod::new("POST")?], Vec::new(), Vec::new()),
        &request
    )?);
    assert!(!matching(
        rule(Vec::new(), vec![query("team", exact("blue"))], Vec::new()),
        &request
    )?);
    assert!(!matching(
        rule(Vec::new(), vec![query("absent", exact("1"))], Vec::new()),
        &request
    )?);
    assert!(!matching(
        rule(Vec::new(), Vec::new(), vec![header("x-env", exact("dev"))?]),
        &request
    )?);
    assert!(!matching(
        rule(Vec::new(), Vec::new(), vec![header("x-absent", exact("1"))?]),
        &request
    )?);
    assert!(CompiledMatchRules::compile(&[rule(Vec::new(), vec![query("a", pattern("("))], Vec::new())]).is_err());
    assert!(
        CompiledMatchRules::compile(&[rule(Vec::new(), Vec::new(), vec![header("x-env", pattern("("))?])]).is_err()
    );
    Ok(())
}

async fn streamed(
    guard: &RequestGuard,
    bytes: &[u8],
) -> TestResult<(ParsedRequest, Vec<u8>, DuplexStream, DuplexStream)> {
    let (mut client, mut server) = duplex(128 * 1024);
    client.write_all(bytes).await?;
    let mut buf = Vec::new();
    let request = read_request_as(&mut server, &mut buf, guard, BodyMode::Stream)
        .await?
        .ok_or("no request was read")?;
    Ok((request, buf, client, server))
}

const SIZED_HEAD: &[u8] = b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 5\r\n\r\n";
const CHUNKED_HEAD: &[u8] = b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nTransfer-Encoding: chunked\r\n\r\n";

#[tokio::test]
async fn a_pending_sized_body_is_read_into_the_request() -> TestResult<()> {
    let guard = guard();
    let mut head = SIZED_HEAD.to_vec();
    head.extend_from_slice(b"hello");
    let (mut request, mut buf, _client, mut server) = streamed(&guard, &head).await?;
    assert!(request.pending.is_some());
    request.hold_body(&mut server, &mut buf, &guard).await?;
    assert!(request.raw_bytes().ends_with(b"\r\n\r\nhello"));
    assert!(request.pending.is_none());
    Ok(())
}

#[tokio::test]
async fn a_pending_chunked_body_is_read_into_the_request() -> TestResult<()> {
    let guard = guard();
    let mut head = CHUNKED_HEAD.to_vec();
    head.extend_from_slice(b"5\r\nhello\r\n0\r\n\r\n");
    let (mut request, mut buf, _client, mut server) = streamed(&guard, &head).await?;
    request.hold_body(&mut server, &mut buf, &guard).await?;
    assert!(request.raw_bytes().ends_with(b"\r\n\r\n5\r\nhello\r\n0\r\n\r\n"));
    Ok(())
}

#[tokio::test]
async fn a_pending_body_over_the_limit_is_refused() -> TestResult<()> {
    let guard = guard().with_body_limit(BodyLimit::new(2));
    let mut sized = SIZED_HEAD.to_vec();
    sized.extend_from_slice(b"hello");
    let (mut request, mut buf, _client, mut server) = streamed(&guard, &sized).await?;
    let outcome = request.hold_body(&mut server, &mut buf, &guard).await;
    assert!(matches!(
        outcome,
        Err(HttpInterceptError::Refused(RefusalError::BodyTooLarge))
    ));

    let mut chunked = CHUNKED_HEAD.to_vec();
    chunked.extend_from_slice(b"5\r\nhello\r\n0\r\n\r\n");
    let (mut request, mut buf, _client, mut server) = streamed(&guard, &chunked).await?;
    let outcome = request.hold_body(&mut server, &mut buf, &guard).await;
    assert!(matches!(
        outcome,
        Err(HttpInterceptError::Refused(RefusalError::BodyTooLarge))
    ));
    Ok(())
}

#[tokio::test]
async fn a_pending_chunked_body_that_is_framed_badly_keeps_its_own_refusal() -> TestResult<()> {
    let guard = guard();
    let mut head = CHUNKED_HEAD.to_vec();
    head.extend_from_slice(b"5\r\nhel");
    let (mut request, mut buf, mut client, mut server) = streamed(&guard, &head).await?;
    client.write_all(b"loXX0\r\n\r\n").await?;
    let outcome = request.hold_body(&mut server, &mut buf, &guard).await;
    assert!(matches!(
        outcome,
        Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
            HeadParseError::ExpectedLfAfterChunkData
        )))
    ));
    Ok(())
}

#[tokio::test]
async fn a_denial_is_logged_and_answered() -> TestResult<()> {
    let _logs = logging();
    let guard = checked_guard(Fixed(RequestDecision::DenyWith(RequestDenial::new())));
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(&get("/x")).await?;
    let mut buf = Vec::new();
    let read = read_checked_request_as(&mut server, &mut buf, &guard, BodyMode::Hold).await?;
    assert!(matches!(read, CheckedRead::Closed));
    drop(server);
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
    Ok(())
}

#[tokio::test]
async fn an_allow_whose_edits_overflow_the_head_is_refused() -> TestResult<()> {
    let edits = HeaderEdits::new().set("X-Pad", "a".repeat(MAX_HEAD_BYTES))?;
    let guard = checked_guard(Fixed(RequestDecision::AllowWith(RequestAllow::new().edits(edits))));
    let (mut client, mut server) = duplex(256 * 1024);
    client.write_all(&get("/x")).await?;
    let mut buf = Vec::new();
    let read = read_checked_request_as(&mut server, &mut buf, &guard, BodyMode::Hold).await?;
    assert!(matches!(read, CheckedRead::Closed));
    drop(server);
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 4"), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_destination_that_is_not_a_host_gets_the_request_denied() -> TestResult<()> {
    let request = parsed(&get("/x")).await?;
    let guard = RequestGuard::new(TlsTarget::new("not a host", 443))
        .with_request_checks(Some(RequestChecks::new(Arc::new(Fixed(RequestDecision::Allow)))));
    let (mut stream, _peer) = duplex(1024);
    let checked = check_request(request, &mut stream, &mut Vec::new(), &guard).await;
    assert!(matches!(checked, Checked::Refuse(RefusalError::RequestDenied)));
    Ok(())
}

#[tokio::test]
async fn a_denial_for_a_target_off_the_default_port_is_reported_with_its_port() -> TestResult<()> {
    let request = parsed(&get("/x")).await?;
    let guard = RequestGuard::new(TlsTarget::new(HOST, 8443))
        .with_request_checks(Some(RequestChecks::new(Arc::new(Fixed(RequestDecision::Deny)))));
    let (mut stream, _peer) = duplex(1024);
    let checked = check_request(request, &mut stream, &mut Vec::new(), &guard).await;
    assert!(matches!(checked, Checked::Refuse(RefusalError::RequestDenied)));
    Ok(())
}

#[tokio::test]
async fn a_filter_reading_the_body_of_an_expecting_request_makes_the_proxy_continue() -> TestResult<()> {
    let guard = checked_guard(ReadsBody);
    let (mut client, mut server) = duplex(64 * 1024);
    client
        .write_all(b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 5\r\nExpect: 100-continue\r\n\r\n")
        .await?;
    let reading = tokio::spawn(async move {
        let mut buf = Vec::new();
        let read = read_checked_request_as(&mut server, &mut buf, &guard, BodyMode::Stream).await?;
        Ok::<_, HttpInterceptError>(match read {
            CheckedRead::Request(request) => request.pending.map(|pending| pending.continued),
            _ => None,
        })
    });
    let mut continued = [0u8; 25];
    tokio::time::timeout(WAIT, client.read_exact(&mut continued)).await??;
    assert_eq!(&continued, b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"hello").await?;
    let pending = tokio::time::timeout(WAIT, reading).await???;
    assert_eq!(pending, Some(true));
    Ok(())
}

fn bad_value_substitution() -> TestResult<(SecretSubstitution, String)> {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(
        SecretName::new("bad")?,
        RealValue::new("bad\nvalue".to_string()),
        InjectHosts::new(vec![DomainPattern::new(HOST)?])?,
    );
    Ok((SecretSubstitution::new(Arc::new(registry)), sentinel.to_string()))
}

#[tokio::test]
async fn a_substitution_the_client_cannot_be_given_is_answered_before_the_mutator_runs() -> TestResult<()> {
    let (substitution, sentinel) = bad_value_substitution()?;
    let target = TlsTarget::new(HOST, 443);
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> = Arc::new(|_: &mut Vec<(String, String)>, _: &Host| {});
    let guard = RequestGuard::new(target.clone())
        .with_substitution(substitution.scope_for(&target))
        .with_request_checks(Some(
            RequestChecks::new(Arc::new(Fixed(RequestDecision::Allow))).with_header_mutator(mutator),
        ));
    let request = parsed_under(
        &guard,
        format!("GET /x HTTP/1.1\r\nHost: {HOST}\r\nX-Key: {sentinel}\r\n\r\n").as_bytes(),
    )
    .await?;
    let (mut client, mut answered) = duplex(64 * 1024);
    let prepared = prepare_upstream_request(&mut client, &request, &guard).await?;
    assert!(matches!(prepared, UpstreamRequest::Refused));
    drop(client);
    let mut answer = String::new();
    answered.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 4"), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_proxied_header_that_substitution_makes_unfit_is_refused() -> TestResult<()> {
    let (substitution, sentinel) = bad_value_substitution()?;
    let target = TlsTarget::new(HOST, 443);
    let scope = substitution.scope_for(&target).ok_or("expected a scope")?;
    let forms = parsed(&get("/x")).await?.forms;
    let mut head = ProxiedHead {
        method: "GET".to_string(),
        target: "/x".to_string(),
        authority: HOST.to_string(),
        host_headers: Vec::new(),
        headers: vec![("X-Key".to_string(), sentinel)],
        framing: Framing::None,
        forms,
    };
    let outcome = substitute_proxied(&mut head, &scope).await;
    assert!(matches!(
        outcome,
        Err(HttpInterceptError::Refused(RefusalError::MalformedRequest(_)))
    ));
    Ok(())
}

#[tokio::test]
async fn a_mutator_that_overflows_the_head_gets_the_request_refused() -> TestResult<()> {
    let mutator: Arc<dyn crate::header_mutator::HeaderMutator> =
        Arc::new(|headers: &mut Vec<(String, String)>, _: &Host| {
            headers.push(("X-Pad".to_string(), "a".repeat(MAX_HEAD_BYTES)));
        });
    let guard = guard().with_request_checks(Some(
        RequestChecks::new(Arc::new(Fixed(RequestDecision::Allow))).with_header_mutator(mutator),
    ));
    let request = parsed(&get("/x")).await?;
    let (mut client, mut answered) = duplex(256 * 1024);
    let prepared = prepare_upstream_request(&mut client, &request, &guard).await?;
    assert!(matches!(prepared, UpstreamRequest::Refused));
    drop(client);
    let mut answer = String::new();
    answered.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 4"), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_request_whose_head_cannot_be_rebuilt_without_its_hop_headers_is_refused() -> TestResult<()> {
    let mut request = parsed(b"GET /x HTTP/1.1\r\nHost: api.example.com\r\nConnection: keep-alive\r\n\r\n").await?;
    request.headers.push(("X-Pad".to_string(), "a".repeat(MAX_HEAD_BYTES)));
    let (mut client, mut answered) = duplex(256 * 1024);
    let prepared = prepare_upstream_request(&mut client, &request, &guard()).await?;
    assert!(matches!(prepared, UpstreamRequest::Refused));
    drop(client);
    let mut answer = String::new();
    answered.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 4"), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_streamed_body_is_rechunked_when_its_length_header_goes_with_the_hop_headers() -> TestResult<()> {
    let guard = guard();
    let (request, _buf, _client, _server) = streamed(
        &guard,
        b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nConnection: content-length\r\nContent-Length: 5\r\n\r\n",
    )
    .await?;
    let (mut client, _answered) = duplex(64 * 1024);
    let prepared = prepare_upstream_request(&mut client, &request, &guard).await?;
    let UpstreamRequest::Rewritten(rewritten) = prepared else {
        return Err("the request was not rewritten".into());
    };
    assert_eq!(
        rewritten.pending.map(|pending| pending.scan),
        Some(Some(Framing::Chunked))
    );
    Ok(())
}

#[tokio::test]
async fn a_body_with_an_encoding_is_left_alone_and_the_skip_is_logged() -> TestResult<()> {
    let _logs = logging();
    let fx = fixture()?;
    let scope = scope(&fx, HOST)?;
    let request =
        parsed(b"POST /u HTTP/1.1\r\nHost: api.example.com\r\nContent-Encoding: gzip\r\nContent-Length: 3\r\n\r\nabc")
            .await?;
    let substituted = substitute_request(&request, &scope, BodyLimit::new(1024))?;
    assert!(substituted.request.is_none());
    Ok(())
}

#[tokio::test]
async fn a_body_without_framing_is_not_rewritten() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, HOST)?;
    let mut request = parsed(&get("/x")).await?;
    request.raw.extend_from_slice(b"stray");
    let substituted = substitute_request(&request, &scope, BodyLimit::new(1024))?;
    assert!(substituted.request.is_none());
    Ok(())
}

#[tokio::test]
async fn a_substitution_that_overflows_the_head_is_refused() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, HOST)?;
    let mut request = parsed(&get("/x")).await?;
    request.headers.push(("Authorization".to_string(), fx.api.clone()));
    request.headers.push(("X-Pad".to_string(), "a".repeat(MAX_HEAD_BYTES)));
    let outcome = substitute_request(&request, &scope, BodyLimit::new(1024));
    assert!(matches!(
        outcome,
        Err(RefusalError::Unparsed(HeadParseError::HeaderOverflow))
    ));
    Ok(())
}

#[test]
fn a_body_without_framing_has_nothing_to_scan() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, HOST)?;
    assert_eq!(plan_body_scan(&mut Vec::new(), Framing::None, &scope), None);
    Ok(())
}

#[test]
fn a_sized_body_whose_held_tail_passes_the_limit_is_refused() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, HOST)?;
    let body = format!("aa{}", &fx.api[..3]);
    let outcome = rewrite_sized_body(body.as_bytes(), &mut scope.scanner(), BodyLimit::new(2));
    assert!(matches!(outcome, Err(RefusalError::BodyTooLarge)));
    Ok(())
}

#[test]
fn a_chunked_body_that_grows_past_the_limit_is_refused() -> TestResult<()> {
    let fx = fixture()?;
    let scope = scope(&fx, HOST)?;
    let tail = format!("4\r\naa{}\r\n0\r\n\r\n", &fx.api[..2]);
    let outcome = rewrite_chunked_body(tail.as_bytes(), &mut scope.scanner(), BodyLimit::new(2));
    assert!(matches!(outcome, Err(RefusalError::BodyTooLarge)));
    let outcome = rewrite_chunked_body(
        format!("c8\r\n{}\r\n0\r\n\r\n", "a".repeat(200)).as_bytes(),
        &mut scope.scanner(),
        BodyLimit::new(1),
    );
    assert!(matches!(outcome, Err(RefusalError::BodyTooLarge)));
    Ok(())
}

#[tokio::test]
async fn a_body_with_no_framing_is_relayed_as_nothing() -> TestResult<()> {
    let mut client: &[u8] = b"";
    let mut upstream = Vec::new();
    let sent = relay_rechunked_body(
        &mut client,
        &mut Vec::new(),
        &mut upstream,
        Framing::None,
        Framing::Chunked,
    )
    .await?;
    assert_eq!(sent, 0);
    Ok(())
}

#[tokio::test]
async fn a_rechunked_body_whose_chunk_is_not_ended_by_a_line_break_is_refused() -> TestResult<()> {
    let mut client: &[u8] = b"5\r\nhelloXY0\r\n\r\n";
    let mut upstream = Vec::new();
    let outcome = relay_rechunked_body(
        &mut client,
        &mut Vec::new(),
        &mut upstream,
        Framing::Chunked,
        Framing::Chunked,
    )
    .await;
    assert!(matches!(
        outcome,
        Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
            HeadParseError::ExpectedLfAfterChunkData
        )))
    ));
    Ok(())
}

#[tokio::test]
async fn a_pending_body_without_a_scanner_is_rechunked_when_its_framing_changed() -> TestResult<()> {
    let guard = guard();
    let (mut request, _buf, _client, _server) = streamed(&guard, SIZED_HEAD).await?;
    request.pending = request.pending.map(|pending| PendingBody {
        scan: Some(Framing::Chunked),
        ..pending
    });
    let mut client: &[u8] = b"hello";
    let mut upstream = Vec::new();
    let sent = relay_body_only(&mut client, &mut Vec::new(), &mut upstream, &request, None).await?;
    assert_eq!(upstream, b"5\r\nhello\r\n0\r\n\r\n");
    assert_eq!(sent, upstream.len() as u64);
    Ok(())
}

async fn line_refusal(line: BodyLine, partial: Vec<u8>) -> TestResult<HeadParseError> {
    let mut empty: &[u8] = b"";
    let mut buf = partial;
    match find_crlf(&mut empty, &mut buf, 0, line).await {
        Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error))) => Ok(error),
        _ => Err("the line was not refused".into()),
    }
}

#[tokio::test]
async fn a_body_line_is_refused_as_soon_as_it_can_only_be_invalid() -> TestResult<()> {
    let mut extension = b"5;".to_vec();
    extension.extend(std::iter::repeat_n(b'a', MAX_CHUNK_EXTENSION_BYTES + 1));
    assert!(matches!(
        line_refusal(BodyLine::ChunkSize, extension).await?,
        HeadParseError::ChunkExtensionsOverflow
    ));

    let digits = vec![b'f'; MAX_REQUEST_BODY_LINE_BYTES + 1];
    assert!(matches!(
        line_refusal(BodyLine::ChunkSize, digits).await?,
        HeadParseError::ChunkExtensionsOverflow
    ));

    assert!(matches!(
        line_refusal(BodyLine::ChunkSize, b"zz".to_vec()).await?,
        HeadParseError::InvalidChunkSize
    ));

    let trailer = vec![b'x'; MAX_REQUEST_TRAILER_BYTES];
    assert!(matches!(
        line_refusal(BodyLine::Trailer(TrailerBytes::default()), trailer).await?,
        HeadParseError::HeaderOverflow
    ));

    let mut blanks = b"a:".to_vec();
    blanks.extend(vec![b' '; MAX_REQUEST_BODY_LINE_BYTES]);
    assert!(matches!(
        line_refusal(BodyLine::Trailer(TrailerBytes::default()), blanks).await?,
        HeadParseError::HeaderOverflow
    ));
    Ok(())
}

#[test]
fn quoted_chunk_extension_values_may_escape_characters() {
    assert!(chunk_extensions_are_valid(b";a=\"x\\\"y\""));
    assert!(!chunk_extensions_are_valid(b";a=\"x\\\x01\""));
}

#[test]
fn a_chunked_message_is_whole_only_once_its_last_chunk_arrived() {
    assert_eq!(buffered_message_len(Framing::Chunked, b"5\r\nab"), None);
    assert_eq!(buffered_message_len(Framing::Chunked, b"0\r\n\r\n"), Some(5));
}

#[test]
fn the_prefix_of_a_chunked_body_is_judged_as_far_as_it_arrived() {
    assert_eq!(chunked_prefix_error(b"ffffffffffffffffffff\r\n"), None);
    assert_eq!(chunked_prefix_error(b"5\r\nhello\r"), None);
    assert_eq!(chunked_prefix_error(b"0\r\nabc"), None);
    assert_eq!(
        chunked_prefix_error(b"5\r\nhelloXX"),
        Some(HeadParseError::ExpectedLfAfterChunkData)
    );
}

#[tokio::test]
async fn a_chunked_start_reads_for_its_first_bytes_when_none_came_with_the_head() -> TestResult<()> {
    let mut stream: &[u8] = b"5\r\nhello\r\n0\r\n\r\n";
    let mut buf = Vec::new();
    check_chunked_start(&mut stream, &mut buf, 0).await?;
    assert!(!buf.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_head_that_never_ends_is_refused_once_it_passes_the_limit() -> TestResult<()> {
    let mut head = b"GET /".to_vec();
    head.extend(std::iter::repeat_n(b'a', REQUEST_HEAD_LIMIT + 1));
    let outcome = parsed_outcome(&head).await;
    assert!(matches!(
        outcome,
        Err(HttpInterceptError::Refused(RefusalError::Unparsed(
            HeadParseError::HeaderOverflow
        )))
    ));
    Ok(())
}

async fn parsed_outcome(bytes: &[u8]) -> Result<Option<ParsedRequest>, HttpInterceptError> {
    let (mut client, mut server) = duplex(128 * 1024);
    client.write_all(bytes).await.map_err(HttpInterceptError::ClientWrite)?;
    let mut buf = Vec::new();
    read_request(&mut server, &mut buf, &guard()).await
}

#[test]
fn a_refused_absolute_form_without_a_path_is_reported_with_the_root_path() {
    assert_eq!(
        refused_target_parts(b"GET http://api.example.com HTTP/1.1\r\n"),
        Some(("GET".to_string(), "api.example.com".to_string(), "/".to_string()))
    );
}

#[test]
fn a_proxied_request_must_be_absolute_form_and_plain_http() {
    let origin = read_proxied_head(b"GET /x HTTP/1.1\r\nHost: a.example\r\n\r\n", AmbiguousTargets::Refuse);
    assert!(matches!(origin, Err(RefusalError::MalformedRequest(_))));
    let secure = read_proxied_head(
        b"GET https://a.example/x HTTP/1.1\r\nHost: a.example\r\n\r\n",
        AmbiguousTargets::Refuse,
    );
    assert!(matches!(secure, Err(RefusalError::MalformedRequest(_))));
}

#[tokio::test]
async fn a_query_credential_is_added_to_the_forwarded_target() -> TestResult<()> {
    let request = parsed(&get("/x?a=1&api_key=old")).await?;
    let injected = inject(
        &request,
        &CredentialInjection::Query("api_key".to_string()),
        &ShortLivedToken::new("tok"),
    );
    let text = String::from_utf8(injected)?;
    assert!(text.starts_with("GET /x?a=1&api_key=tok HTTP/1.1\r\n"), "{text}");
    Ok(())
}

#[tokio::test]
async fn a_response_that_fails_before_its_head_is_answered_with_a_bad_gateway() -> TestResult<()> {
    let mut upstream: &[u8] = b"";
    let mut client = Vec::new();
    relay_response_or_bad_gateway(
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
    assert!(String::from_utf8(client)?.starts_with("HTTP/1.1 502 "));
    Ok(())
}

/// An upstream that takes `writable` writes and fails the next one, and that
/// answers with `response` once a write failed.
struct Breaking {
    writable: usize,
    failed: bool,
    waker: Option<Waker>,
    response: Vec<u8>,
    served: usize,
}

impl Breaking {
    fn new(writable: usize, response: &[u8]) -> Self {
        Self {
            writable,
            failed: false,
            waker: None,
            response: response.to_vec(),
            served: 0,
        }
    }
}

impl AsyncRead for Breaking {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if !self.failed {
            self.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let from = self.served;
        let take = (self.response.len() - from).min(buf.remaining());
        let piece = self.response[from..from + take].to_vec();
        buf.put_slice(&piece);
        self.served += take;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Breaking {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.writable == 0 {
            self.failed = true;
            if let Some(waker) = self.waker.take() {
                waker.wake();
            }
            return Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe)));
        }
        self.writable -= 1;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_head_the_fresh_upstream_could_not_take_is_answered_with_a_bad_gateway() -> TestResult<()> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    client.write_all(&get("/x")).await?;
    client.shutdown().await?;
    let guard = guard();
    let ended = run_compiled_http_intercept(
        proxy_client,
        UpstreamLeg::connected(Breaking::new(0, b"")),
        None,
        &guard,
    )
    .await;
    assert!(matches!(ended, Err(HttpInterceptError::UpstreamWrite(_))));
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    Ok(())
}

#[tokio::test]
async fn an_upstream_that_stops_taking_the_body_still_has_its_answer_relayed() -> TestResult<()> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let mut request = SIZED_HEAD.to_vec();
    request.extend_from_slice(b"hello");
    client.write_all(&request).await?;
    let upstream = Breaking::new(1, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    let guard = guard();
    let counts = run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(upstream), None, &guard).await?;
    assert_eq!(counts.client_to_upstream, SIZED_HEAD.len() as u64);
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_body_framed_badly_after_its_head_went_upstream_ends_the_connection_without_an_answer() -> TestResult<()> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream) = duplex(64 * 1024);
    let proxy = tokio::spawn(async move {
        let guard = guard();
        run_compiled_http_intercept(proxy_client, UpstreamLeg::connected(proxy_upstream), None, &guard)
            .await
            .map(|_| ())
    });
    let mut head = CHUNKED_HEAD.to_vec();
    head.extend_from_slice(b"5\r\nhel");
    client.write_all(&head).await?;
    let mut seen = Vec::new();
    let mut chunk = [0u8; 256];
    while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = tokio::time::timeout(WAIT, upstream.read(&mut chunk)).await??;
        if read == 0 {
            return Err("the upstream was closed".into());
        }
        seen.extend_from_slice(&chunk[..read]);
    }
    client.write_all(b"loXX0\r\n\r\n").await?;
    tokio::time::timeout(WAIT, proxy).await???;
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.is_empty(), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_head_a_redialable_upstream_could_not_take_leaves_the_tunnel_open() -> TestResult<()> {
    let (mut client, proxy_client) = duplex(64 * 1024);
    client.write_all(&get("/x")).await?;
    client.shutdown().await?;
    let dial: crate::upstream_leg::Dial<Breaking> = Box::new(|| Box::pin(async { Ok(Breaking::new(0, b"")) }));
    let guard = guard();
    run_compiled_http_intercept(proxy_client, UpstreamLeg::on_demand(dial), None, &guard).await?;
    let mut answer = String::new();
    client.read_to_string(&mut answer).await?;
    assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    Ok(())
}

#[tokio::test]
async fn a_pending_body_that_declares_no_framing_holds_nothing() -> TestResult<()> {
    let guard = guard();
    let (mut request, mut buf, _client, mut server) = streamed(&guard, SIZED_HEAD).await?;
    request.pending = request.pending.map(|pending| PendingBody {
        incoming: Framing::None,
        ..pending
    });
    request.hold_body(&mut server, &mut buf, &guard).await?;
    assert!(request.pending.is_none());
    Ok(())
}
