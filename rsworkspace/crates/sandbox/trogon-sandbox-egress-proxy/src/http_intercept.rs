//! Minimal HTTP/1.1 request parsing, request filtering,
//! [`trogon_sandbox_contract::MatchRule`] evaluation, and credential injection over
//! an already-decrypted TLS pair (see
//! [`crate::tls_intercept::terminate_and_forward`]). This is deliberately not
//! a general-purpose HTTP library: just enough parsing of the request line,
//! headers, and a `Content-Length` or chunked body to filter, match, and apply
//! one safe header/query rewrite, then relay everything else (the response,
//! and any further keep-alive requests on the same connection) byte-for-byte.
//!
//! Every request is checked by a [`RequestGuard`] before it is matched or
//! forwarded: echo methods are refused, the request-target must be
//! unambiguous, the `Host` header must name the TLS target, and the body is
//! bounded. A refused request is answered with an explicit HTTP response (see
//! [`RefusalError::status`]) and the connection is closed.
//!

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use regex::Regex;
use trogon_sandbox_contract::AmbiguousTargets;
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::ResponseHeaderStrip;
use trogon_sandbox_protocol::HostMatch;

pub use crate::body_limit::BodyLimit;
use crate::chunked_body::{decode_chunked_body, parse_chunk_size};
use crate::connection_headers::{asks_to_close, asks_to_keep_alive};
use crate::constants::{MAX_HEAD_BYTES, READ_CHUNK_BYTES};
use crate::framing::Framing;
pub use crate::head_parse_error::HeadParseError;
pub use crate::host_mismatch_error::HostMismatchError;
pub use crate::http_intercept_error::HttpInterceptError;
pub use crate::invalid_match_rule_error::InvalidMatchRuleError;
pub use crate::refusal_error::RefusalError;
use crate::request_version::RequestVersion;
use crate::response_head::{find_header_terminator, has_opaque_content_encoding};
use crate::sni::ClientServerName;
pub use crate::status_code::StatusCode;
use crate::stream_use::StreamUse;
pub use crate::tls_target::TlsTarget;
use crate::upstream_leg::{AfterDiscard, UpstreamLeg};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use trogon_sandbox_contract::SecretName;
use trogon_sandbox_contract::{
    CredentialInjection, CredentialTransform, HeaderMatcher, HttpMethod, MatchRule, OidcToken, QueryMatcher,
    RefusedRequest, StringMatcher,
};
use url::Url;

use crate::credential_source::CredentialSource;
use crate::early_answer::{QuietReads, UntilAnswered, UpstreamAfter};
use crate::framed_head::{AnsweredRequest, FramedAnswer};
use crate::header_mutator::mutated;
use crate::header_syntax::is_header_value;
use crate::request_body::{RequestBody, StreamedBody};
use crate::request_decision::{RequestAllow, RequestDenial, ResponseHook};
use crate::request_filter::{
    RequestChecks, RequestDecision, RequestScheme, RequestView, TargetForms, without_hop_by_hop,
};
use crate::request_line::{LineEnd, PRI, Preface, RequestLine};
use crate::response_relay::{self, Connection, HopByHop, Relayed, ResponseScrub};
use crate::secret_substitution::{Scanner, SubstitutionScope};
use crate::sigv4_resign::{PairRefusal, ResignPlan, Sigv4BodyLimit, Sigv4RefusalError};
use crate::tunnel_deadlines::TunnelEndError;

mod constants;
pub(crate) use constants::{
    CLIENT_READ_BYTES, FORWARDED_HOST_HEADER, FORWARDED_PATH_HEADER, FORWARDED_PORT_HEADER, FORWARDED_SCHEME_HEADER,
    REQUEST_HEAD_LIMIT, SANDBOX_OIDC_TOKEN_HEADER,
};
use constants::{MAX_CHUNK_EXTENSION_BYTES, MAX_REQUEST_BODY_LINE_BYTES, MAX_REQUEST_TRAILER_BYTES, READY_BYTES_LIMIT};

/// What became of the tunnel when a request was answered without being
/// forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterAnswer {
    Open,
    Ended,
    Destroyed,
}

/// What reading the next checked request came to: a request to forward, a
/// request answered with the tunnel left open for the next one, or the end
/// of the tunnel (answered or not).
pub(crate) enum CheckedRead {
    Request(Box<ParsedRequest>),
    Answered,
    Closed,
}

/// The body limit one request is held to and what refuses it: a body the proxy
/// must hash to re-sign has its own, larger limit and refusal.
#[derive(Debug, Clone)]
struct RequestBodyLimit {
    limit: BodyLimit,
    refusal: RefusalError,
}

impl RequestBodyLimit {
    fn refuse(self, error: HttpInterceptError) -> HttpInterceptError {
        match error {
            HttpInterceptError::Refused(RefusalError::BodyTooLarge) => HttpInterceptError::Refused(self.refusal),
            other => other,
        }
    }
}

/// By when the first request's whole head must be in, counted the way the workload's HTTP stack counts
/// it from the ClientHello, and the window that is counted in steps of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FirstHead {
    by: tokio::time::Instant,
    window: Duration,
}

impl FirstHead {
    /// The first deadline after the handshake: one that passes while the
    /// handshake is still under way is moved on by another window, as the workload's HTTP stack's
    /// timer is.
    pub(crate) fn counted_from(hello_at: tokio::time::Instant, window: Duration) -> Self {
        let now = tokio::time::Instant::now();
        let mut by = hello_at + window;
        while by <= now && !window.is_zero() {
            by += window;
        }
        Self { by, window }
    }
}

/// What came of waiting for a request's head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextHead {
    /// The head is in, or what is there is left to the request reader to judge.
    Arrived,
    /// The client closed the connection between requests.
    Ended,
    /// The deadline passed and the tunnel was ended.
    Idle,
}

/// Per-connection request checks applied to every decrypted request.
#[derive(Debug, Clone)]
pub struct RequestGuard {
    target: TlsTarget,
    body_limit: BodyLimit,
    body_capped: bool,
    network_strip: Option<Arc<ResponseHeaderStrip>>,
    sigv4_body_limit: Sigv4BodyLimit,
    substitution: Option<SubstitutionScope>,
    checks: Option<RequestChecks>,
    keep_alive: Option<Duration>,
    ended_idle: Arc<AtomicBool>,
    destroyed: Arc<AtomicBool>,
    first_head: Option<FirstHead>,
    client_sni: Option<String>,
    server_name: ClientServerName,
    ambiguous_targets: AmbiguousTargets,
    host_match: HostMatch,
}

impl RequestGuard {
    pub fn new(target: TlsTarget) -> Self {
        Self {
            target,
            body_limit: BodyLimit::DEFAULT,
            body_capped: false,
            network_strip: None,
            sigv4_body_limit: Sigv4BodyLimit::DEFAULT,
            substitution: None,
            checks: None,
            keep_alive: None,
            ended_idle: Arc::new(AtomicBool::new(false)),
            destroyed: Arc::new(AtomicBool::new(false)),
            first_head: None,
            client_sni: None,
            server_name: ClientServerName::Absent,
            ambiguous_targets: AmbiguousTargets::Refuse,
            host_match: HostMatch::Required,
        }
    }

    pub fn with_host_match(mut self, host_match: HostMatch) -> Self {
        self.host_match = host_match;
        self
    }

    pub(crate) fn with_client_server_name(mut self, server_name: ClientServerName) -> Self {
        self.server_name = server_name;
        self
    }

    fn check_server_name(&self) -> Result<(), RefusalError> {
        match &self.server_name {
            ClientServerName::Absent => Ok(()),
            ClientServerName::Name(name) if name.eq_ignore_ascii_case(self.target.host()) => Ok(()),
            ClientServerName::Name(_) => Err(RefusalError::HostMismatch(HostMismatchError::ServerName)),
            ClientServerName::Unreadable => Err(RefusalError::HostMismatch(HostMismatchError::ServerNameUnreadable)),
        }
    }

    pub(crate) fn with_client_sni(mut self, sni: Option<String>) -> Self {
        self.client_sni = sni;
        self
    }

    pub(crate) fn with_keep_alive(mut self, keep_alive: Duration) -> Self {
        self.keep_alive = Some(keep_alive);
        self
    }

    /// How long a client may take to answer the end of a tunnel left idle,
    /// as long again as the keep-alive time.
    pub(crate) fn grace_after_idle_end(&self) -> Option<Duration> {
        self.ended_idle
            .load(Ordering::Relaxed)
            .then_some(self.keep_alive)
            .flatten()
    }

    /// Whether the tunnel ended on a request the proxy destroys the socket for, with
    /// nothing written and nothing read off it, so that what the client still
    /// sends turns the close into a reset.
    pub(crate) fn ended_destroyed(&self) -> bool {
        self.destroyed.load(Ordering::Relaxed)
    }

    pub(crate) fn no_first_request(&self) -> HttpInterceptError {
        let window = self.first_head.map(|first| first.window).unwrap_or_default();
        HttpInterceptError::ClientRead(TunnelEndError::NoFirstRequest(window).into())
    }

    pub(crate) fn with_first_head(mut self, first: FirstHead) -> Self {
        self.first_head = Some(first);
        self
    }

    /// Waits until `buf` holds the whole head of the next request: the first
    /// by the first-request deadline, a later one within the keep-alive time
    /// of `idle_since`, the end of the last response. Part of a head does not
    /// move either deadline. A tunnel left idle is ended with a close_notify,
    /// once its idle time is up.
    pub(crate) async fn next_head<S>(
        &self,
        stream: &mut S,
        buf: &mut Vec<u8>,
        idle_since: Option<tokio::time::Instant>,
    ) -> NextHead
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let (deadline, window) = match (idle_since, self.first_head, self.keep_alive) {
            (Some(since), _, Some(keep_alive)) => (since + keep_alive, keep_alive),
            (None, Some(first), _) => (first.by, first.window),
            _ => return NextHead::Arrived,
        };
        let mut chunk = vec![0u8; CLIENT_READ_BYTES];
        while find_header_terminator(buf).is_none() && buf.len() <= REQUEST_HEAD_LIMIT {
            match tokio::time::timeout_at(deadline, stream.read(&mut chunk)).await {
                Ok(Ok(0)) if buf.is_empty() => return NextHead::Ended,
                Ok(Ok(0)) | Ok(Err(_)) => return NextHead::Arrived,
                Ok(Ok(read)) => buf.extend_from_slice(&chunk[..read]),
                Err(_) => {
                    let _ = tokio::time::timeout(window, stream.shutdown()).await;
                    self.ended_idle.store(true, Ordering::Relaxed);
                    return NextHead::Idle;
                }
            }
        }
        NextHead::Arrived
    }

    pub(crate) fn with_request_checks(mut self, checks: Option<RequestChecks>) -> Self {
        self.checks = checks;
        self
    }

    pub(crate) fn with_substitution(mut self, substitution: Option<SubstitutionScope>) -> Self {
        self.substitution = substitution;
        self
    }

    pub(crate) fn substitution(&self) -> Option<&SubstitutionScope> {
        self.substitution.as_ref()
    }

    /// The response scrub for a request whose substitution scope is `scope`.
    pub(crate) fn response_scrub_for<'a>(&'a self, scope: Option<&'a SubstitutionScope>) -> Option<ResponseScrub<'a>> {
        ResponseScrub::new(scope, self.network_strip.as_deref(), self.body_limit)
    }

    pub(crate) fn with_network_strip(mut self, strip: Option<Arc<ResponseHeaderStrip>>) -> Self {
        self.network_strip = strip;
        self
    }

    pub fn with_sigv4_body_limit(mut self, limit: Sigv4BodyLimit) -> Self {
        self.sigv4_body_limit = limit;
        self
    }

    fn resigns_body(scope: Option<&SubstitutionScope>, headers: &[(String, String)], target: &str) -> bool {
        scope
            .and_then(|scope| scope.plan_sigv4(headers, target).ok().flatten())
            .is_some_and(|plan| plan.hashes_body())
    }

    fn request_body_limit(
        &self,
        scope: Option<&SubstitutionScope>,
        headers: &[(String, String)],
        target: &str,
    ) -> RequestBodyLimit {
        if Self::resigns_body(scope, headers, target) {
            RequestBodyLimit {
                limit: BodyLimit::new(self.sigv4_body_limit.bytes()),
                refusal: RefusalError::Sigv4(Sigv4RefusalError::BodyTooLarge(self.sigv4_body_limit)),
            }
        } else {
            RequestBodyLimit {
                limit: self.body_limit,
                refusal: RefusalError::BodyTooLarge,
            }
        }
    }

    pub fn with_body_limit(mut self, body_limit: BodyLimit) -> Self {
        self.body_limit = body_limit;
        self.body_capped = true;
        self
    }

    /// Whether a request body that nothing needs to hold is relayed as it
    /// arrives, without a size limit.
    pub(crate) fn streams_bodies(&self) -> bool {
        !self.body_capped
    }

    pub fn with_ambiguous_targets(mut self, mode: AmbiguousTargets) -> Self {
        self.ambiguous_targets = mode;
        self
    }

    fn target_mode(&self) -> AmbiguousTargets {
        if self.checks.is_some() {
            self.ambiguous_targets
        } else {
            AmbiguousTargets::AsSpelled
        }
    }
}

enum CompiledStringMatcher {
    Exact(String),
    StartsWith(String),
    Regex(Regex),
}

impl CompiledStringMatcher {
    fn compile(matcher: &StringMatcher) -> Result<Self, InvalidMatchRuleError> {
        Ok(match matcher {
            StringMatcher::Exact(expected) => Self::Exact(expected.clone()),
            StringMatcher::StartsWith(prefix) => Self::StartsWith(prefix.clone()),
            StringMatcher::Regex(pattern) => {
                Self::Regex(Regex::new(pattern).map_err(|source| InvalidMatchRuleError {
                    pattern: pattern.clone(),
                    source,
                })?)
            }
        })
    }

    fn is_match(&self, value: &str) -> bool {
        match self {
            Self::Exact(expected) => value == expected,
            Self::StartsWith(prefix) => value.starts_with(prefix.as_str()),
            Self::Regex(re) => re.is_match(value),
        }
    }
}

struct CompiledQueryMatcher {
    key: String,
    value: CompiledStringMatcher,
}

struct CompiledHeaderMatcher {
    name: String,
    value: CompiledStringMatcher,
}

struct CompiledMatchRule {
    path: Option<CompiledStringMatcher>,
    methods: Vec<String>,
    query_string: Vec<CompiledQueryMatcher>,
    headers: Vec<CompiledHeaderMatcher>,
}

impl CompiledMatchRule {
    fn compile(rule: &MatchRule) -> Result<Self, InvalidMatchRuleError> {
        Ok(Self {
            path: rule.path.as_ref().map(CompiledStringMatcher::compile).transpose()?,
            methods: rule.methods.iter().map(|method| method.as_str().to_string()).collect(),
            query_string: rule
                .query_string
                .iter()
                .map(|QueryMatcher { key, value }| {
                    Ok(CompiledQueryMatcher {
                        key: key.clone(),
                        value: CompiledStringMatcher::compile(value)?,
                    })
                })
                .collect::<Result<_, InvalidMatchRuleError>>()?,
            headers: rule
                .headers
                .iter()
                .map(|HeaderMatcher { name, value }| {
                    Ok(CompiledHeaderMatcher {
                        name: name.as_str().to_string(),
                        value: CompiledStringMatcher::compile(value)?,
                    })
                })
                .collect::<Result<_, InvalidMatchRuleError>>()?,
        })
    }

    fn matches(&self, request: &ParsedRequest) -> bool {
        if let Some(path_matcher) = &self.path
            && !path_matcher.is_match(&request.path)
        {
            return false;
        }

        if !self.methods.is_empty() && !self.methods.iter().any(|m| m == &request.method) {
            return false;
        }

        let query_ok = self.query_string.iter().all(|matcher| {
            request
                .query
                .iter()
                .any(|(key, value)| key == &matcher.key && matcher.value.is_match(value))
        });
        if !query_ok {
            return false;
        }

        self.headers.iter().all(|matcher| {
            request
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case(&matcher.name) && matcher.value.is_match(value))
        })
    }
}

/// [`MatchRule`]s with every `Regex` pattern compiled once, so a request is
/// matched without compiling anything and an invalid pattern is rejected
/// before any request is handled. OR semantics across rules: an empty set
/// matches every request.
pub struct CompiledMatchRules {
    rules: Vec<CompiledMatchRule>,
}

impl CompiledMatchRules {
    pub fn compile(match_rules: &[MatchRule]) -> Result<Self, InvalidMatchRuleError> {
        Ok(Self {
            rules: match_rules
                .iter()
                .map(CompiledMatchRule::compile)
                .collect::<Result<_, _>>()?,
        })
    }

    pub(crate) fn matches(&self, request: &ParsedRequest) -> bool {
        self.rules.is_empty() || self.rules.iter().any(|rule| rule.matches(request))
    }
}

/// Byte counts relayed in each direction, mirroring
/// [`tokio::io::copy_bidirectional`]'s return shape so callers can report the
/// same `ProxyOutcome::Completed` metrics regardless of which path ran.
#[derive(Debug, Default, Clone, Copy)]
pub struct RelayCounts {
    pub client_to_upstream: u64,
    pub upstream_to_client: u64,
}

/// Drives one TLS-terminated connection end to end: reads HTTP/1.1 requests
/// off `client`, refuses any that fail `guard`, evaluates
/// `transform.match_rules` per request, injects a broker-issued credential
/// into matched requests before forwarding to `upstream`, relays the response
/// back unmodified, and loops until either side closes.
pub async fn run_http_intercept<C, U>(
    client: C,
    upstream: U,
    transform: &CredentialTransform,
    credential_source: &dyn CredentialSource,
    guard: &RequestGuard,
) -> Result<RelayCounts, HttpInterceptError>
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let match_rules = CompiledMatchRules::compile(&transform.match_rules)?;
    run_compiled_http_intercept(
        client,
        UpstreamLeg::connected(upstream),
        Some(Injection {
            transform,
            match_rules: &match_rules,
            credential_source,
        }),
        guard,
    )
    .await
}

pub(crate) struct Injection<'a> {
    pub(crate) transform: &'a CredentialTransform,
    pub(crate) match_rules: &'a CompiledMatchRules,
    pub(crate) credential_source: &'a dyn CredentialSource,
}

pub(crate) async fn run_compiled_http_intercept<C, U>(
    mut client: C,
    mut upstream: UpstreamLeg<U>,
    injection: Option<Injection<'_>>,
    guard: &RequestGuard,
) -> Result<RelayCounts, HttpInterceptError>
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let mut counts = RelayCounts::default();
    let mut client_buf: Vec<u8> = Vec::new();
    let mode = if injection.is_none() && guard.streams_bodies() {
        BodyMode::Stream
    } else {
        BodyMode::Hold
    };

    let mut idle_since = None;
    loop {
        match guard.next_head(&mut client, &mut client_buf, idle_since).await {
            NextHead::Arrived => {}
            NextHead::Ended => break,
            NextHead::Idle => match idle_since {
                Some(_) => break,
                None => return Err(guard.no_first_request()),
            },
        }
        let request = match read_checked_request_as(&mut client, &mut client_buf, guard, mode).await? {
            CheckedRead::Request(request) => *request,
            CheckedRead::Answered => {
                idle_since = Some(tokio::time::Instant::now());
                continue;
            }
            CheckedRead::Closed => break,
        };

        let prepared = match prepare_upstream_request(&mut client, &request, guard).await? {
            UpstreamRequest::Refused => break,
            prepared => prepared,
        };
        let to_upstream = prepared.resolve(&request);

        let outgoing = match &injection {
            Some(injection) if injection.match_rules.matches(&request) => {
                let token = injection
                    .credential_source
                    .fetch(&injection.transform.credential)
                    .await?;
                inject(to_upstream, &injection.transform.injection, &token)
            }
            _ => to_upstream.raw.clone(),
        };

        let (upstream_stream, stream_use) = match upstream.stream().await {
            Ok(taken) => taken,
            Err(error) => {
                let closes = request.closes_connection() || !upstream.can_redial();
                answer_lost_upstream(&mut client, &mut client_buf, to_upstream, closes, error).await?;
                idle_since = Some(tokio::time::Instant::now());
                continue;
            }
        };
        // The response is awaited next, so a TLS record left buffered here
        // would never reach the upstream.
        let sent_head = async {
            upstream_stream.write_all(&outgoing).await?;
            upstream_stream.flush().await
        };
        if let Err(error) = sent_head.await {
            let error = HttpInterceptError::UpstreamWrite(error);
            if error.ends_silently(stream_use) {
                return Err(error);
            }
            let closes = request.closes_connection() || upstream.discard() == AfterDiscard::Ends;
            answer_lost_upstream(&mut client, &mut client_buf, to_upstream, closes, error).await?;
            idle_since = Some(tokio::time::Instant::now());
            continue;
        }
        counts.client_to_upstream += outgoing.len() as u64;
        let exchanged = match exchange(
            &mut client,
            &mut client_buf,
            upstream_stream,
            to_upstream,
            request.substitution(),
            &request.method,
            guard.response_scrub_for(request.substitution()),
            request.response_hook(),
            request.expects_continue(),
            request.answered(),
            stream_use,
        )
        .await
        {
            Ok(exchanged) => exchanged,
            Err(HttpInterceptError::Refused(refusal)) => {
                answer_refusal(&mut client, request.answered(), refusal).await?;
                break;
            }
            Err(error) => return Err(error),
        };
        counts.client_to_upstream += exchanged.body_bytes;
        counts.upstream_to_client += exchanged.relayed.bytes;
        idle_since = Some(exchanged.answered_at);
        let leg_ends = match exchanged.upstream {
            UpstreamAfter::Spent => upstream.discard() == AfterDiscard::Ends,
            UpstreamAfter::Reusable => {
                upstream.served();
                false
            }
        };
        if leg_ends
            || exchanged.client_body == ClientBody::Unread
            || exchanged.relayed.connection == Connection::Close
            || request.closes_connection()
        {
            break;
        }
    }

    Ok(counts)
}

#[derive(Clone)]
pub(crate) struct ParsedRequest {
    method: String,
    version: RequestVersion,
    target: String,
    forms: TargetForms,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    /// The request as it is forwarded when unmatched: the checked
    /// request-target, the headers, and the body with its original framing.
    raw: Vec<u8>,
    header_block_len: usize,
    response_hook: Option<ResponseHook>,
    pending: Option<PendingBody>,
    /// The connection's substitution scope narrowed to this request's method
    /// and path, so a binding scoped to either is replaced only here.
    substitution: Option<SubstitutionScope>,
}

/// A request body still on the client's connection: `raw` holds the head
/// only, and the body is relayed as it arrives.
#[derive(Debug, Clone, Copy)]
struct PendingBody {
    incoming: Framing,
    scan: Option<Framing>,
    continued: bool,
}

impl ParsedRequest {
    /// The substitution scope for this request: the guard's connection-level
    /// scope narrowed by this request's method and path.
    pub(crate) fn substitution(&self) -> Option<&SubstitutionScope> {
        self.substitution.as_ref()
    }

    pub(crate) fn response_hook(&self) -> Option<&ResponseHook> {
        self.response_hook.as_ref()
    }

    /// Reads the body off the client so `raw` carries it, for what has to
    /// see the whole request. The body is held to the guard's limit.
    pub(crate) async fn hold_body<S>(
        &mut self,
        stream: &mut S,
        buf: &mut Vec<u8>,
        guard: &RequestGuard,
    ) -> Result<(), HttpInterceptError>
    where
        S: AsyncRead + Unpin,
    {
        let Some(pending) = self.pending else {
            return Ok(());
        };
        let body_limit = guard.request_body_limit(self.substitution.as_ref(), &self.headers, &self.target);
        let end = match pending.incoming {
            Framing::None => 0,
            Framing::ContentLength(length) => {
                if !body_limit.limit.allows(length) {
                    return Err(HttpInterceptError::Refused(body_limit.refusal));
                }
                fill_to(stream, buf, length).await?;
                length
            }
            Framing::Chunked => read_chunked_body(stream, buf, 0, body_limit.limit)
                .await
                .map_err(|error| body_limit.refuse(error))?,
        };
        self.raw.extend(buf.drain(..end));
        self.pending = None;
        Ok(())
    }

    /// The request with an allow's header edits applied and its response hook
    /// attached.
    fn with_allow(mut self, allow: &RequestAllow) -> Result<Self, RefusalError> {
        if !allow.header_edits().is_empty() {
            let mut headers = self.headers.clone();
            allow.header_edits().apply(&mut headers);
            self = self
                .with_headers(headers)
                .ok_or(RefusalError::Unparsed(HeadParseError::HeaderOverflow))?;
        }
        self.response_hook = allow.response_hook().cloned();
        Ok(self)
    }

    /// The request with `headers` in place of its own, or `None` when they
    /// do not fit the head limit.
    fn with_headers(&self, headers: Vec<(String, String)>) -> Option<Self> {
        let body = &self.raw[self.header_block_len..];
        let raw = render_request(&self.method, &self.target, &headers, body);
        let header_block_len = raw.len() - body.len();
        if header_block_len > MAX_HEAD_BYTES {
            return None;
        }
        Some(Self {
            headers,
            raw,
            header_block_len,
            ..self.clone()
        })
    }

    /// [`Self::with_headers`] for headers that no longer frame the body the
    /// way `declared` did: a body held in `raw` is re-chunked here, one still
    /// on the client is re-chunked as it is relayed.
    fn reframed_with(
        &self,
        headers: Vec<(String, String)>,
        declared: Framing,
        reframed: bool,
    ) -> Result<Self, RefusalError> {
        let mut rewritten = self
            .with_headers(headers)
            .ok_or(RefusalError::Unparsed(HeadParseError::HeaderOverflow))?;
        if !reframed {
            return Ok(rewritten);
        }
        match (&mut rewritten.pending, declared) {
            (Some(pending), Framing::ContentLength(_)) => pending.scan = Some(Framing::Chunked),
            (None, Framing::ContentLength(_)) => {
                let body = rewritten.raw[rewritten.header_block_len..].to_vec();
                rewritten.raw.truncate(rewritten.header_block_len);
                rewritten.raw.extend(chunk_whole(&body));
            }
            _ => {}
        }
        Ok(rewritten)
    }

    pub(crate) fn expects_continue(&self) -> bool {
        expects_continue(&self.headers)
    }

    /// Whether the client's connection ends after this request: an HTTP/1.1
    /// request keeps it unless it asks to close, an HTTP/1.0 one only when
    /// it asks to keep it.
    pub(crate) fn closes_connection(&self) -> bool {
        match self.version {
            RequestVersion::Http11 => asks_to_close(&self.headers),
            RequestVersion::Http10 => !asks_to_keep_alive(&self.headers),
        }
    }

    pub(crate) fn method(&self) -> &str {
        &self.method
    }

    /// What a strict HTTP server knows of this request when it answers it.
    pub(crate) fn answered(&self) -> AnsweredRequest {
        AnsweredRequest::of_version(&self.method, self.version, self.closes_connection())
    }

    /// The bytes forwarded for an unmodified request.
    pub(crate) fn raw_bytes(&self) -> &[u8] {
        &self.raw
    }
}

/// [`read_request`], answering a refused request on `stream` and ending the
/// connection (`Ok(None)`) so callers only deal with requests that passed the
/// guard. A request answered with the connection left open is `Ok(None)` too.
#[cfg(test)]
pub(crate) async fn read_checked_request<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    guard: &RequestGuard,
) -> Result<Option<ParsedRequest>, HttpInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    Ok(
        match read_checked_request_as(stream, buf, guard, BodyMode::Hold).await? {
            CheckedRead::Request(request) => Some(*request),
            CheckedRead::Answered | CheckedRead::Closed => None,
        },
    )
}

/// How the body of a request is taken off the client: held in `raw` with the
/// head, or left on the connection for the caller to relay as it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyMode {
    Hold,
    Stream,
}

pub(crate) async fn read_checked_request_as<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    guard: &RequestGuard,
    mode: BodyMode,
) -> Result<CheckedRead, HttpInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    match read_request_as(stream, buf, guard, mode).await {
        Err(HttpInterceptError::Refused(refusal)) => Ok(match answer_refused_head(stream, buf, refusal).await? {
            AfterAnswer::Open => CheckedRead::Answered,
            AfterAnswer::Ended => CheckedRead::Closed,
            AfterAnswer::Destroyed => {
                guard.destroyed.store(true, Ordering::Relaxed);
                CheckedRead::Closed
            }
        }),
        Ok(Some(request)) => {
            let answered = request.answered();
            match check_request(request, stream, buf, guard).await {
                Checked::Forward(request) => Ok(CheckedRead::Request(request)),
                Checked::Refuse(refusal) => {
                    answer_refusal(stream, answered, refusal).await?;
                    Ok(CheckedRead::Closed)
                }
                Checked::Deny(denial) => {
                    answer_denial(stream, answered, &denial).await?;
                    Ok(CheckedRead::Closed)
                }
            }
        }
        Ok(None) => Ok(CheckedRead::Closed),
        Err(error) => Err(error),
    }
}

enum Checked {
    Forward(Box<ParsedRequest>),
    Refuse(RefusalError),
    Deny(RequestDenial),
}

async fn check_request<S>(
    mut request: ParsedRequest,
    stream: &mut S,
    buf: &mut Vec<u8>,
    guard: &RequestGuard,
) -> Checked
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let Some(checks) = &guard.checks else {
        return Checked::Forward(Box::new(request));
    };
    let (Ok(host), Ok(method)) = (Host::new(guard.target.host()), HttpMethod::new(&request.method)) else {
        return Checked::Refuse(RefusalError::RequestDenied);
    };
    let chunked = matches!(request_framing(&request.headers), Ok(Framing::Chunked));
    let continued = AtomicBool::new(false);
    let query = request.target.split_once('?').map(|(_, query)| query);
    let decision = match request.pending {
        Some(pending) => {
            let source = StreamedBody::new(
                &mut *stream,
                &mut *buf,
                pending.incoming,
                expects_continue(&request.headers),
                &continued,
            );
            checks
                .decide(&RequestView {
                    host: &host,
                    port: guard.target.port(),
                    method: &method,
                    path: &request.path,
                    query,
                    sni: guard.client_sni.as_deref(),
                    headers: &request.headers,
                    body: Some(RequestBody::streamed(&source)),
                    scheme: RequestScheme::Https,
                    forms: Some(&request.forms),
                })
                .await
        }
        None => {
            checks
                .decide(&RequestView {
                    host: &host,
                    port: guard.target.port(),
                    method: &method,
                    path: &request.path,
                    query,
                    sni: guard.client_sni.as_deref(),
                    headers: &request.headers,
                    body: Some(RequestBody::new(&request.raw[request.header_block_len..], chunked)),
                    scheme: RequestScheme::Https,
                    forms: Some(&request.forms),
                })
                .await
        }
    };
    if continued.load(Ordering::SeqCst)
        && let Some(pending) = request.pending.as_mut()
    {
        pending.continued = true;
    }
    let reason = match &decision {
        RequestDecision::DenyWith(denial) => denial.denial_reason().to_string(),
        _ => RequestDenial::new().denial_reason().to_string(),
    };
    let denied = match decision {
        RequestDecision::Allow => return Checked::Forward(Box::new(request)),
        RequestDecision::AllowWith(allow) => {
            return match request.with_allow(&allow) {
                Ok(edited) => Checked::Forward(Box::new(edited)),
                Err(refusal) => Checked::Refuse(refusal),
            };
        }
        RequestDecision::DenyWith(denial) => Checked::Deny(denial),
        _ => Checked::Refuse(RefusalError::RequestDenied),
    };
    let port = guard.target.port();
    let authority = match port {
        443 => guard.target.host().to_string(),
        port => format!("{}:{port}", guard.target.host()),
    };
    let refused = RefusedRequest::new(
        &request.method,
        &format!("https://{authority}{}", request.target),
        &reason,
    );
    checks.report_denied(guard.target.host(), port, refused).await;
    denied
}

pub(crate) async fn answer_denial<S>(
    stream: &mut S,
    request: AnsweredRequest,
    denial: &RequestDenial,
) -> Result<(), HttpInterceptError>
where
    S: AsyncWrite + Unpin,
{
    tracing::warn!(
        status = denial.answer_status(),
        reason = denial.denial_reason(),
        "denied intercepted request"
    );
    stream
        .write_all(&denial.answer(request, SystemTime::now()))
        .await
        .map_err(HttpInterceptError::ClientWrite)?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// The 502 for an upstream that could not be reached. a strict HTTP server keeps the
/// connection open after it unless the request asked to close.
pub(crate) async fn answer_bad_gateway<C>(client: &mut C, request: AnsweredRequest) -> Result<(), HttpInterceptError>
where
    C: AsyncWrite + Unpin,
{
    client
        .write_all(&bad_gateway_bytes(request, SystemTime::now()))
        .await
        .map_err(HttpInterceptError::ClientWrite)?;
    client.flush().await.map_err(HttpInterceptError::ClientWrite)
}

/// Answers a request whose upstream could not be dialed or written with
/// [`answer_bad_gateway`] and drops what is left of its body. `Err` ends the
/// tunnel, closed first when `closes`.
pub(crate) async fn answer_lost_upstream<C>(
    client: &mut C,
    buf: &mut Vec<u8>,
    forwarded: &ParsedRequest,
    closes: bool,
    error: HttpInterceptError,
) -> Result<(), HttpInterceptError>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    let request = AnsweredRequest::of_version(forwarded.method(), forwarded.version, closes);
    let closes = !bad_gateway_answer().keeps_alive(request);
    answer_bad_gateway(client, request).await?;
    drain_pending_body(client, buf, forwarded).await;
    if closes {
        let _ = client.shutdown().await;
        return Err(error);
    }
    tracing::warn!(%error, "upstream unreachable; the tunnel stays open");
    Ok(())
}

pub(crate) async fn answer_refusal<S>(
    stream: &mut S,
    request: AnsweredRequest,
    refusal: RefusalError,
) -> Result<(), HttpInterceptError>
where
    S: AsyncWrite + Unpin,
{
    tracing::warn!(
        status = refusal.status().as_u16(),
        reason = %refusal,
        "refused intercepted request"
    );
    stream
        .write_all(&refusal.answer(request, SystemTime::now()))
        .await
        .map_err(HttpInterceptError::ClientWrite)?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// [`answer_refusal`] for a request refused by its head, read on to the end
/// of its body first so that a client still sending it is not reset. A refusal
/// the proxy answers without ending the tunnel leaves it open when the request's
/// body was read to its end and the client did not ask to close.
async fn answer_refused_head<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    refusal: RefusalError,
) -> Result<AfterAnswer, HttpInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if refusal.ends_unanswered() {
        tracing::warn!(reason = %refusal, "refused intercepted request");
        return Ok(AfterAnswer::Destroyed);
    }
    let head = find_header_terminator(buf).map(|end| buf.drain(..end).collect::<Vec<u8>>());
    let framing = head.as_deref().and_then(framing_of_refused_head);
    let stays = refusal.keeps_tunnel()
        && framing.is_some()
        && head
            .as_deref()
            .is_some_and(|head| AnsweredRequest::of_head(head).keeps_alive());
    tracing::warn!(
        status = refusal.status().as_u16(),
        reason = %refusal,
        "refused intercepted request"
    );
    let request = head
        .as_deref()
        .map_or(AnsweredRequest::new("GET", true), AnsweredRequest::of_head);
    let response = refusal.answer(request, SystemTime::now());
    stream
        .write_all(&response)
        .await
        .map_err(HttpInterceptError::ClientWrite)?;
    stream.flush().await.map_err(HttpInterceptError::ClientWrite)?;
    let drained = match framing {
        Some(framing) => drain_body_whole(stream, buf, framing).await,
        None => false,
    };
    if stays && drained {
        return Ok(AfterAnswer::Open);
    }
    let _ = stream.shutdown().await;
    Ok(AfterAnswer::Ended)
}

/// The request as it goes to the connection's real destination: the one the
/// client sent, or a copy with sentinels substituted. `Refused` means the
/// client was already answered and the connection must end.
pub(crate) enum UpstreamRequest {
    Unchanged,
    Rewritten(Box<ParsedRequest>),
    Refused,
}

impl UpstreamRequest {
    pub(crate) fn resolve<'a>(&'a self, original: &'a ParsedRequest) -> &'a ParsedRequest {
        match self {
            Self::Rewritten(rewritten) => rewritten,
            Self::Unchanged | Self::Refused => original,
        }
    }
}

/// Substitutes the lease's sentinels in `request` for the guard's TLS target.
/// The sentinels of bindings that do not inject at the target stay in place
/// and are reported. A request that cannot be rewritten within the body
/// limit is answered and refused rather than forwarded half-substituted.
pub(crate) async fn prepare_upstream_request<S>(
    client: &mut S,
    request: &ParsedRequest,
    guard: &RequestGuard,
) -> Result<UpstreamRequest, HttpInterceptError>
where
    S: AsyncWrite + Unpin,
{
    let hop_free = match forwardable(request, &guard.target) {
        Some(Ok(rewritten)) => Some(rewritten),
        Some(Err(refusal)) => {
            answer_refusal(client, request.answered(), refusal).await?;
            return Ok(UpstreamRequest::Refused);
        }
        None => None,
    };
    let prepared = prepare_with_mutator(client, hop_free.as_ref().unwrap_or(request), guard).await?;
    Ok(match (prepared, hop_free) {
        (UpstreamRequest::Unchanged, Some(hop_free)) => UpstreamRequest::Rewritten(Box::new(hop_free)),
        (prepared, _) => prepared,
    })
}

/// The request as it is forwarded once the hop-by-hop headers were stripped and `Host` was
/// rebuilt from the tunnel's target, or
/// `None` when that changes nothing. A body whose framing headers went with
/// them is framed as chunked again, the way it is left to the upstream
/// leg.
fn forwardable(request: &ParsedRequest, target: &TlsTarget) -> Option<Result<ParsedRequest, RefusalError>> {
    let declared = request_framing(&request.headers).unwrap_or(Framing::None);
    let mut headers = without_hop_by_hop(&request.headers).into_owned();
    let authority = target.authority();
    match headers.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case("host")) {
        Some((_, value)) => *value = authority,
        None => headers.push(("Host".to_string(), authority)),
    }
    let reframed = declared != Framing::None && matches!(request_framing(&headers), Ok(Framing::None));
    if reframed {
        headers.push(("Transfer-Encoding".to_string(), "chunked".to_string()));
    }
    if headers == request.headers {
        return None;
    }
    Some(request.reframed_with(headers, declared, reframed))
}

async fn prepare_with_mutator<S>(
    client: &mut S,
    request: &ParsedRequest,
    guard: &RequestGuard,
) -> Result<UpstreamRequest, HttpInterceptError>
where
    S: AsyncWrite + Unpin,
{
    let prepared = substitute_upstream_request(client, request, guard).await?;
    let Some(mutator) = guard.checks.as_ref().and_then(RequestChecks::mutator) else {
        return Ok(prepared);
    };
    if matches!(prepared, UpstreamRequest::Refused) {
        return Ok(prepared);
    }
    let current = prepared.resolve(request);
    let edited = Host::new(guard.target.host())
        .ok()
        .and_then(|destination| mutated(mutator, &current.headers, &destination).ok())
        .map(|headers| {
            if headers == current.headers {
                Ok(None)
            } else {
                current
                    .with_headers(headers)
                    .map(Some)
                    .ok_or(RefusalError::Unparsed(HeadParseError::HeaderOverflow))
            }
        });
    match edited {
        Some(Ok(None)) => Ok(prepared),
        Some(Ok(Some(rewritten))) => Ok(UpstreamRequest::Rewritten(Box::new(rewritten))),
        Some(Err(refusal)) => {
            answer_refusal(client, request.answered(), refusal).await?;
            Ok(UpstreamRequest::Refused)
        }
        None => {
            answer_refusal(client, request.answered(), RefusalError::RequestDenied).await?;
            Ok(UpstreamRequest::Refused)
        }
    }
}

async fn substitute_upstream_request<S>(
    client: &mut S,
    request: &ParsedRequest,
    guard: &RequestGuard,
) -> Result<UpstreamRequest, HttpInterceptError>
where
    S: AsyncWrite + Unpin,
{
    let Some(scope) = request.substitution() else {
        return Ok(UpstreamRequest::Unchanged);
    };
    let plan = match scope.plan_sigv4(&request.headers, &request.target) {
        Ok(plan) => plan,
        Err(refused) => {
            scope.report_sigv4_refused(&refused).await;
            answer_refusal(client, request.answered(), RefusalError::Sigv4(refused.refusal)).await?;
            return Ok(UpstreamRequest::Refused);
        }
    };
    let substitute_limit = if plan.as_ref().is_some_and(|plan| plan.hashes_body()) {
        BodyLimit::new(guard.sigv4_body_limit.bytes())
    } else {
        guard.body_limit
    };
    let substituted = match substitute_request(request, scope, substitute_limit) {
        Ok(substituted) => substituted,
        Err(refusal) => {
            answer_refusal(client, request.answered(), refusal).await?;
            return Ok(UpstreamRequest::Refused);
        }
    };
    scope.report(&substituted.misdirected).await;
    let Some(plan) = plan else {
        return Ok(match substituted.request {
            Some(rewritten) => UpstreamRequest::Rewritten(Box::new(rewritten)),
            None => UpstreamRequest::Unchanged,
        });
    };
    match resign_request(substituted.request.as_ref().unwrap_or(request), &plan) {
        Ok(resigned) => Ok(UpstreamRequest::Rewritten(Box::new(resigned))),
        Err(refusal) => {
            if let RefusalError::Sigv4(sigv4) = &refusal {
                let pair = PairRefusal {
                    refusal: sigv4.clone(),
                    binding: plan.binding().clone(),
                };
                scope.report_sigv4_refused(&pair).await;
            }
            answer_refusal(client, request.answered(), refusal).await?;
            Ok(UpstreamRequest::Refused)
        }
    }
}

/// Signs `request`, the one that goes upstream after substitution, with the
/// real credential. The payload hash covers the body as the upstream decodes
/// it, so a chunked body is decoded first. The body was held to the re-signing
/// limit when it was read and when it was substituted.
fn resign_request(request: &ParsedRequest, plan: &ResignPlan) -> Result<ParsedRequest, RefusalError> {
    let body = &request.raw[request.header_block_len..];
    let payload = if !plan.hashes_body() {
        Cow::Borrowed(&[][..])
    } else if matches!(request_framing(&request.headers)?, Framing::Chunked) {
        Cow::Owned(decode_chunked_body(body)?)
    } else {
        Cow::Borrowed(body)
    };

    let mut headers = request.headers.clone();
    plan.apply(&request.method, &request.target, &mut headers, &payload)
        .map_err(RefusalError::Sigv4)?;
    request
        .with_headers(headers)
        .ok_or(RefusalError::Unparsed(HeadParseError::HeaderOverflow))
}

/// A header value after substitution, or the refusal of one that is no longer
/// a value a header can carry.
fn substituted_header(out: Vec<u8>) -> Result<String, RefusalError> {
    let value =
        String::from_utf8(out).map_err(|_| RefusalError::MalformedRequest("substituted header is not valid UTF-8"))?;
    if !is_header_value(&value) {
        return Err(RefusalError::MalformedRequest(
            "substituted header holds a character a header cannot carry",
        ));
    }
    Ok(value)
}

struct Substituted {
    request: Option<ParsedRequest>,
    misdirected: BTreeSet<SecretName>,
}

fn substitute_request(
    request: &ParsedRequest,
    scope: &SubstitutionScope,
    body_limit: BodyLimit,
) -> Result<Substituted, RefusalError> {
    let mut misdirected = BTreeSet::new();
    let mut replaced = 0;
    let mut downgraded = false;

    let mut headers = Vec::with_capacity(request.headers.len());
    for (name, value) in &request.headers {
        if scope.injects_here() && name.eq_ignore_ascii_case("accept-encoding") {
            downgraded |= !value.eq_ignore_ascii_case("identity");
            headers.push((name.clone(), "identity".to_string()));
            continue;
        }
        let mut scanner = scope.scanner();
        let mut out = Vec::with_capacity(value.len());
        scanner.feed(value.as_bytes(), &mut out);
        scanner.finish(&mut out);
        replaced += scanner.replaced();
        misdirected.append(&mut scanner.take_flagged());
        headers.push((name.clone(), substituted_header(out)?));
    }

    let body = &request.raw[request.header_block_len..];
    let mut scan = None;
    let rewritten_body = if let Some(pending) = request.pending {
        scan = plan_body_scan(&mut headers, pending.incoming, scope);
        None
    } else if body.is_empty() {
        None
    } else if has_opaque_content_encoding(&request.headers) {
        tracing::warn!(
            host = scope.target().host(),
            port = scope.target().port(),
            "request body has a Content-Encoding, secret substitution skipped for the body"
        );
        None
    } else {
        let mut scanner = scope.scanner();
        let rewritten = match request_framing(&request.headers)? {
            Framing::None => None,
            Framing::ContentLength(_) => Some(rewrite_sized_body(body, &mut scanner, body_limit)?),
            Framing::Chunked => Some(rewrite_chunked_body(body, &mut scanner, body_limit)?),
        };
        replaced += scanner.replaced();
        misdirected.append(&mut scanner.take_flagged());
        rewritten
    };

    if replaced == 0 && !downgraded && scan.is_none() {
        return Ok(Substituted {
            request: None,
            misdirected,
        });
    }

    let body = rewritten_body.as_deref().unwrap_or(body);
    if request.pending.is_none() && matches!(request_framing(&request.headers)?, Framing::ContentLength(_)) {
        for (name, value) in &mut headers {
            if name.eq_ignore_ascii_case("content-length") {
                *value = body.len().to_string();
            }
        }
    }

    let raw = render_request(&request.method, &request.target, &headers, body);
    let header_block_len = raw.len() - body.len();
    if header_block_len > MAX_HEAD_BYTES {
        return Err(RefusalError::Unparsed(HeadParseError::HeaderOverflow));
    }
    Ok(Substituted {
        request: Some(ParsedRequest {
            headers,
            raw,
            header_block_len,
            pending: request.pending.map(|pending| PendingBody { scan, ..pending }),
            ..request.clone()
        }),
        misdirected,
    })
}

/// Substitutes the lease's sentinels in the headers of a plain-HTTP request
/// the client sent to the proxy, and says how its body is to be scanned as it
/// is relayed: `None` when it is passed on untouched. Nothing here re-signs:
/// SigV4 stays on terminated connections.
pub(crate) async fn substitute_proxied(
    head: &mut ProxiedHead,
    scope: &SubstitutionScope,
) -> Result<Option<Framing>, HttpInterceptError> {
    let mut misdirected = BTreeSet::new();
    let mut headers = Vec::with_capacity(head.headers.len());
    for (name, value) in &head.headers {
        let mut scanner = scope.scanner();
        let mut out = Vec::with_capacity(value.len());
        scanner.feed(value.as_bytes(), &mut out);
        scanner.finish(&mut out);
        misdirected.append(&mut scanner.take_flagged());
        headers.push((
            name.clone(),
            substituted_header(out).map_err(HttpInterceptError::Refused)?,
        ));
    }
    let scan = plan_body_scan(&mut headers, head.framing, scope);
    head.headers = headers;
    scope.report(&misdirected).await;
    Ok(scan)
}

fn chunk_whole(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 16);
    if !body.is_empty() {
        out.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

/// How a request body is scanned for sentinels as it is relayed, the way
/// body substitution does: not at all when it has none or
/// is encoded, and re-framed as chunked when a substitution can change its
/// length, since the declared length would no longer be true.
fn plan_body_scan(
    headers: &mut Vec<(String, String)>,
    incoming: Framing,
    scope: &SubstitutionScope,
) -> Option<Framing> {
    if incoming == Framing::None {
        return None;
    }
    let outgoing = request_framing(headers).unwrap_or(incoming);
    if has_opaque_content_encoding(headers) {
        tracing::warn!(
            host = scope.target().host(),
            port = scope.target().port(),
            "request body has a Content-Encoding, secret substitution skipped for the body"
        );
        return (outgoing != incoming).then_some(outgoing);
    }
    if matches!(outgoing, Framing::ContentLength(_)) && !scope.keeps_body_length() {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-length"));
        headers.push(("Transfer-Encoding".to_string(), "chunked".to_string()));
        return Some(Framing::Chunked);
    }
    Some(outgoing)
}

fn rewrite_sized_body(body: &[u8], scanner: &mut Scanner<'_>, limit: BodyLimit) -> Result<Vec<u8>, RefusalError> {
    let mut out = Vec::with_capacity(body.len());
    for chunk in body.chunks(READ_CHUNK_BYTES) {
        scanner.feed(chunk, &mut out);
        if !limit.allows(out.len()) {
            return Err(RefusalError::BodyTooLarge);
        }
    }
    scanner.finish(&mut out);
    if !limit.allows(out.len()) {
        return Err(RefusalError::BodyTooLarge);
    }
    Ok(out)
}

/// Re-frames a chunked body after substitution: each chunk's payload is fed
/// through `scanner` (so a sentinel split across chunks is still found) and
/// whatever it releases is re-emitted as one chunk. Chunk extensions are
/// dropped and so are trailers.
fn rewrite_chunked_body(body: &[u8], scanner: &mut Scanner<'_>, limit: BodyLimit) -> Result<Vec<u8>, RefusalError> {
    let malformed = || RefusalError::MalformedRequest("malformed chunked body");
    let mut out = Vec::with_capacity(body.len());
    let mut released = 0usize;
    let mut pos = 0;
    loop {
        let line_end = body[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|offset| pos + offset)
            .ok_or_else(malformed)?;
        let size_text = body[pos..line_end].split(|b| *b == b';').next().unwrap_or_default();
        let size = std::str::from_utf8(size_text)
            .ok()
            .and_then(|text| usize::from_str_radix(text, 16).ok())
            .ok_or_else(malformed)?;
        pos = line_end + 2;
        if size == 0 {
            break;
        }
        let payload = body.get(pos..pos + size).ok_or_else(malformed)?;
        pos += size + 2;

        let mut piece = Vec::new();
        scanner.feed(payload, &mut piece);
        released += piece.len();
        if !limit.allows(released) {
            return Err(RefusalError::BodyTooLarge);
        }
        push_chunk(&mut out, &piece);
    }

    let mut tail = Vec::new();
    scanner.finish(&mut tail);
    released += tail.len();
    if !limit.allows(released) {
        return Err(RefusalError::BodyTooLarge);
    }
    push_chunk(&mut out, &tail);
    out.extend_from_slice(b"0\r\n\r\n");
    Ok(out)
}

fn push_chunk(out: &mut Vec<u8>, payload: &[u8]) {
    if payload.is_empty() {
        return;
    }
    out.extend_from_slice(format!("{:x}\r\n", payload.len()).as_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(b"\r\n");
}

/// Reads one HTTP/1.1 request (request line, headers, and a `Content-Length`
/// or chunked body) off `stream`, buffering into `buf`. Bytes read past the
/// end of the request stay in `buf` for the next call. Returns `None` on a
/// clean EOF before any bytes of a new request arrive, and
/// [`HttpInterceptError::Refused`] for a request that fails `guard`.
#[cfg(test)]
pub(crate) async fn read_request<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    guard: &RequestGuard,
) -> Result<Option<ParsedRequest>, HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    read_request_as(stream, buf, guard, BodyMode::Hold).await
}

/// [`read_request`] that, in [`BodyMode::Stream`], reads the head only and
/// leaves the body on the connection, except where the request has to be
/// held: a SigV4 request that signs a literal body hash is re-signed over the
/// whole body.
pub(crate) async fn read_request_as<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    guard: &RequestGuard,
    mode: BodyMode,
) -> Result<Option<ParsedRequest>, HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    let header_block_len = match read_until_header_end(stream, buf).await? {
        Some(len) => len,
        None => return Ok(None),
    };

    let header_text = std::str::from_utf8(&buf[..header_block_len])
        .map_err(|_| RefusalError::MalformedRequest("bytes that are not UTF-8"))
        .map_err(HttpInterceptError::Refused)?;
    if let Some(Preface::Refused(error)) = Preface::of(buf) {
        return Err(HttpInterceptError::Refused(RefusalError::Unparsed(error)));
    }
    let parts = parse_head_parts(header_text, guard.target_mode().into()).map_err(HttpInterceptError::Refused)?;
    if parts.framing != Framing::None {
        read_ready(stream, buf).await;
    }
    if parts.framing == Framing::Chunked
        && let Some(error) = chunked_prefix_error(&buf[header_block_len..])
    {
        return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error)));
    }
    let keeps_alive = AnsweredRequest::of_head(&buf[..header_block_len]).keeps_alive();
    let head = check_host(parts, guard).map_err(HttpInterceptError::Refused)?;

    let (path, query) = split_target(&head.target);
    let substitution = guard.substitution().map(|scope| scope.for_request(&head.method, &path));
    let body_limit = guard.request_body_limit(substitution.as_ref(), &head.headers, &head.target);
    let streamed =
        mode == BodyMode::Stream && !RequestGuard::resigns_body(substitution.as_ref(), &head.headers, &head.target);
    let body_end = match head.framing {
        Framing::None => header_block_len,
        _ if streamed => header_block_len,
        Framing::ContentLength(length) => {
            if !body_limit.limit.allows(length) {
                return Err(HttpInterceptError::Refused(body_limit.refusal));
            }
            let end = header_block_len + length;
            fill_to(stream, buf, end).await?;
            end
        }
        Framing::Chunked => read_chunked_body(stream, buf, header_block_len, body_limit.limit)
            .await
            .map_err(|error| body_limit.refuse(error))?,
    };

    if streamed && head.framing == Framing::Chunked && !expects_continue(&head.headers) {
        check_chunked_start(stream, buf, header_block_len).await?;
    }
    let message_end = if streamed {
        buffered_message_len(head.framing, &buf[header_block_len..])
    } else {
        Some(body_end - header_block_len)
    };
    if let Some(error) = message_end.and_then(|end| pipelined_error(keeps_alive, &buf[header_block_len + end..])) {
        return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error)));
    }

    let consumed: Vec<u8> = buf.drain(..body_end).collect();
    let body = &consumed[header_block_len..];

    let raw = render_request(&head.method, &head.target, &head.headers, body);
    let header_block_len = raw.len() - body.len();

    Ok(Some(ParsedRequest {
        method: head.method,
        version: head.version,
        target: head.target,
        forms: head.forms,
        path,
        query,
        headers: head.headers,
        raw,
        header_block_len,
        response_hook: None,
        pending: (streamed && head.framing != Framing::None).then_some(PendingBody {
            incoming: head.framing,
            scan: None,
            continued: false,
        }),
        substitution,
    }))
}

pub(crate) fn expects_continue(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("expect") && value.eq_ignore_ascii_case("100-continue"))
}

struct RequestHead {
    method: String,
    version: RequestVersion,
    target: String,
    forms: TargetForms,
    headers: Vec<(String, String)>,
    framing: Framing,
}

struct HeadParts {
    method: String,
    version: RequestVersion,
    headers: Vec<(String, String)>,
    framing: Framing,
    target: RequestTarget,
}

fn check_host(parts: HeadParts, guard: &RequestGuard) -> Result<RequestHead, RefusalError> {
    let host_headers: Vec<&str> = host_header_values(&parts.headers).collect();
    if guard.host_match.is_required() {
        guard.check_server_name()?;
        guard
            .target
            .check(&host_headers, parts.target.authority.as_deref(), parts.version)?;
    } else if host_headers.is_empty() && parts.version == RequestVersion::Http11 {
        return Err(RefusalError::MissingHost);
    }

    Ok(RequestHead {
        method: parts.method,
        version: parts.version,
        target: parts.target.spelled,
        forms: parts.target.forms,
        headers: parts.headers,
        framing: parts.framing,
    })
}

fn host_header_values(headers: &[(String, String)]) -> impl Iterator<Item = &str> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.as_str())
}

/// The head of a request a client sent to the proxy itself, in absolute-form,
/// checked the way a terminated request is: the same method, framing and
/// request-target refusals apply. The destination is whatever the target
/// names, so the caller decides whether it may be reached.
#[derive(Debug)]
pub(crate) struct ProxiedHead {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) authority: String,
    pub(crate) host_headers: Vec<String>,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) framing: Framing,
    pub(crate) forms: TargetForms,
}

/// Which form of the request-target is forwarded: the one a mode of
/// `ambiguousRequestTarget` picks, or, for a proxy with nothing judging the
/// request, the path and query of the parsed URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetMode {
    Judged(AmbiguousTargets),
    ParsedUrl,
}

impl From<AmbiguousTargets> for TargetMode {
    fn from(mode: AmbiguousTargets) -> Self {
        Self::Judged(mode)
    }
}

/// What a refused request-target is reported as: the method, the authority
/// without its userinfo, and the target as spelled with the fragment cut and
/// every byte outside ASCII percent-encoded.
pub(crate) fn refused_target_parts(head: &[u8]) -> Option<(String, String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut line = text.lines().next()?.split(' ');
    let method = line.next()?;
    let raw = line.next()?;
    let ascii = percent_encode_non_ascii(raw.split('#').next().unwrap_or_default());
    let (authority, remainder) = split_absolute_form(&ascii);
    let authority = authority?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let spelled = if remainder.is_empty() || remainder.starts_with('?') {
        format!("/{remainder}")
    } else {
        remainder.to_string()
    };
    Some((method.to_string(), authority.to_string(), spelled))
}

pub(crate) fn read_proxied_head(head: &[u8], mode: impl Into<TargetMode>) -> Result<ProxiedHead, RefusalError> {
    let mode = mode.into();
    let header_text = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(_) => {
            return Err(match parse_head_parts(&String::from_utf8_lossy(head), mode) {
                Err(RefusalError::TargetBytes) => RefusalError::TargetBytes,
                _ => RefusalError::MalformedRequest("bytes that are not UTF-8"),
            });
        }
    };
    let parts = parse_head_parts(header_text, mode)?;
    let Some(authority) = parts.target.authority else {
        return Err(RefusalError::MalformedRequest(
            "a proxied request must use an absolute-form target",
        ));
    };
    if parts.target.secure {
        return Err(RefusalError::MalformedRequest(
            "only http absolute-form targets are proxied",
        ));
    }
    let host_headers = host_header_values(&parts.headers).map(str::to_string).collect();
    Ok(ProxiedHead {
        method: parts.method,
        target: parts.target.spelled,
        authority,
        host_headers,
        headers: parts.headers,
        framing: parts.framing,
        forms: parts.target.forms,
    })
}

fn parse_head_parts(header_text: &str, mode: TargetMode) -> Result<HeadParts, RefusalError> {
    let (request_line, end, rest) = RequestLine::split(header_text);
    let RequestLine {
        method,
        target: raw_target,
        version,
        ..
    } = RequestLine::read(request_line, end).map_err(RefusalError::unparsed)?;
    let lines = rest.split("\r\n");

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            return Err(RefusalError::Unparsed(HeadParseError::WhitespaceAfterHeaderValue));
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(RefusalError::Unparsed(HeadParseError::InvalidHeaderToken))?;
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(RefusalError::Unparsed(HeadParseError::InvalidHeaderToken));
        }
        headers.push((name.to_string(), value.trim().to_string()));
    }

    let framing = request_framing(&headers)?;

    if method.eq_ignore_ascii_case("TRACE") {
        return Err(RefusalError::EchoMethod);
    }

    let target = RequestTarget::read(raw_target, method, mode)?;

    if method == PRI {
        return Err(RefusalError::Upgrade);
    }
    Ok(HeadParts {
        method: method.to_string(),
        version,
        headers,
        framing,
        target,
    })
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// The body framing a strict HTTP parser reads from the headers, one at a time in
/// their order: a `Content-Length` and a `Transfer-Encoding` refuse each
/// other by whichever came second, and `chunked` may only be the last coding
/// of the last `Transfer-Encoding`. A `Transfer-Encoding` that does not end in
/// `chunked` is refused only once the head is complete, after a strict HTTP server
/// has handed the request to the proxy.
fn request_framing(headers: &[(String, String)]) -> Result<Framing, RefusalError> {
    let mut length = None;
    let mut encoded = false;
    let mut chunked = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(RefusalError::Unparsed(HeadParseError::DuplicateContentLength));
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(RefusalError::Unparsed(HeadParseError::InvalidContentLength));
            }
            if encoded {
                return Err(RefusalError::Unparsed(
                    HeadParseError::ContentLengthAfterTransferEncoding,
                ));
            }
            length = Some(value.parse::<usize>().map_err(|_| RefusalError::BodyTooLarge)?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if length.is_some() {
                return Err(RefusalError::Unparsed(
                    HeadParseError::TransferEncodingAfterContentLength,
                ));
            }
            if value.is_empty() {
                return Err(RefusalError::Unparsed(HeadParseError::EmptyTransferEncoding));
            }
            let mut codings = value.split(',').map(|coding| coding.trim_matches([' ', '\t']));
            let last = codings.next_back();
            if chunked || codings.any(|coding| coding.eq_ignore_ascii_case("chunked")) {
                return Err(RefusalError::Unparsed(HeadParseError::InvalidTransferEncodingValue));
            }
            encoded = true;
            chunked = last.is_some_and(|coding| coding.eq_ignore_ascii_case("chunked"));
        }
    }
    match length {
        _ if chunked => Ok(Framing::Chunked),
        _ if encoded => Err(RefusalError::UnparsedBody(HeadParseError::UnchunkedTransferEncoding)),
        Some(length) => Ok(Framing::ContentLength(length)),
        None => Ok(Framing::None),
    }
}

/// The request-target of a request, read for a terminated request:
/// the form that would be forwarded as spelled, refused when it can name a
/// different resource once normalized.
struct RequestTarget {
    spelled: String,
    forms: TargetForms,
    authority: Option<String>,
    secure: bool,
}

impl RequestTarget {
    fn read(raw: &str, method: &str, mode: TargetMode) -> Result<Self, RefusalError> {
        let without_fragment = raw.split('#').next().unwrap_or_default();
        let ascii = percent_encode_non_ascii(without_fragment);
        let parsed = parse_request_target(&ascii, method).ok_or(RefusalError::MalformedTarget)?;
        let normalized = parsed.normalized();

        let (authority, remainder) = split_absolute_form(&ascii);
        let spelled = if remainder.is_empty() || remainder.starts_with('?') {
            format!("/{remainder}")
        } else {
            remainder.to_string()
        };
        let well_formed =
            spelled == "*" || (spelled.starts_with('/') && spelled.bytes().all(|b| (b'!'..=b'~').contains(&b)));
        if !well_formed {
            return Err(RefusalError::MalformedTarget);
        }
        if without_fragment.contains('\u{fffd}') {
            return Err(RefusalError::TargetBytes);
        }

        let spelled_form = percent_encode_chars(&spelled, "\"'<>^`{}");
        let normalized_form = percent_encode_chars(&normalized, "\"'<>^`{}");
        let ambiguous = spelled_form != normalized_form && spelled_form != format!("{normalized_form}?");
        let forms = TargetForms {
            spelled: spelled.clone(),
            normalized: normalized.clone(),
            ambiguous,
        };
        let forwarded = match mode {
            TargetMode::ParsedUrl => parsed.as_parsed(),
            TargetMode::Judged(AmbiguousTargets::Normalize) => normalized,
            TargetMode::Judged(AmbiguousTargets::AsSpelled) => spelled,
            TargetMode::Judged(AmbiguousTargets::Refuse) if ambiguous => {
                return Err(RefusalError::AmbiguousTarget);
            }
            TargetMode::Judged(AmbiguousTargets::Refuse) => spelled,
        };

        let secure = ascii
            .get(.."https://".len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
        Ok(Self {
            spelled: forwarded,
            forms,
            authority: authority.map(str::to_string),
            secure,
        })
    }
}

fn percent_encode_non_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let mut utf8 = [0u8; 4];
            for byte in ch.encode_utf8(&mut utf8).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

fn percent_encode_chars(text: &str, chars: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if chars.contains(ch) {
            out.push_str(&format!("%{:02X}", ch as u32));
        } else {
            out.push(ch);
        }
    }
    out
}

fn split_absolute_form(target: &str) -> (Option<&str>, &str) {
    let after_scheme = ["http://", "https://"].iter().find_map(|scheme| {
        target
            .get(..scheme.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(scheme))
            .map(|_| &target[scheme.len()..])
    });
    match after_scheme {
        Some(rest) => {
            let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
            if end == 0 {
                (None, target)
            } else {
                (Some(&rest[..end]), &rest[end..])
            }
        }
        None => (None, target),
    }
}

struct ParsedTarget {
    path: String,
    query: Option<String>,
}

impl ParsedTarget {
    fn as_parsed(&self) -> String {
        self.join(&self.path)
    }

    fn normalized(&self) -> String {
        if self.path == "*" {
            return self.path.clone();
        }
        self.join(&format!("/{}", self.path.trim_start_matches('/')))
    }

    fn join(&self, path: &str) -> String {
        match &self.query {
            Some(query) => format!("{path}?{query}"),
            None => path.to_string(),
        }
    }
}

fn parse_request_target(raw: &str, method: &str) -> Option<ParsedTarget> {
    if raw == "*" {
        return (method == "OPTIONS").then(|| ParsedTarget {
            path: "*".to_string(),
            query: None,
        });
    }
    let parse_input = if raw.starts_with('/') {
        format!("http://origin.invalid{raw}")
    } else {
        raw.to_string()
    };
    let url = Url::parse(&parse_input).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some(ParsedTarget {
        path: url.path().to_string(),
        query: url.query().filter(|query| !query.is_empty()).map(str::to_string),
    })
}

async fn fill_more<S>(stream: &mut S, buf: &mut Vec<u8>) -> Result<(), HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    let mut chunk = vec![0u8; CLIENT_READ_BYTES];
    let n = stream.read(&mut chunk).await.map_err(HttpInterceptError::ClientRead)?;
    if n == 0 {
        return Err(HttpInterceptError::MalformedRequest(
            "connection closed mid-request".into(),
        ));
    }
    buf.extend_from_slice(&chunk[..n]);
    Ok(())
}

/// Reads in what the client has already sent, without waiting for more: the workload's HTTP stack
/// parses everything a read brings in one go, so a bad chunk, extension or
/// trailer that arrived with the head is refused before the proxy's handler has
/// sent anything upstream.
pub(crate) async fn read_ready<S>(stream: &mut S, buf: &mut Vec<u8>)
where
    S: AsyncRead + Unpin,
{
    let mut chunk = vec![0u8; CLIENT_READ_BYTES];
    while buf.len() < READY_BYTES_LIMIT {
        let room = chunk.len().min(READY_BYTES_LIMIT - buf.len());
        match tokio::time::timeout(Duration::ZERO, stream.read(&mut chunk[..room])).await {
            Ok(Ok(read)) if read > 0 => buf.extend_from_slice(&chunk[..read]),
            _ => return,
        }
    }
}

async fn fill_to<S>(stream: &mut S, buf: &mut Vec<u8>, len: usize) -> Result<(), HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    while buf.len() < len {
        fill_more(stream, buf).await?;
    }
    Ok(())
}

/// Reads the chunked body that starts at `start` in `buf` and returns the
/// offset just past its last trailer line. The decoded size is bounded by
/// `limit`; the chunk framing stays in `buf` so the body is forwarded as sent.
async fn read_chunked_body<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    start: usize,
    limit: BodyLimit,
) -> Result<usize, HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    let mut pos = start;
    let mut decoded = 0usize;

    loop {
        let line_end = find_crlf(stream, buf, pos, BodyLine::ChunkSize).await?;
        let size = read_chunk_line(&buf[pos..line_end])?;
        let canonical = format!("{size:x}");
        buf.splice(pos..line_end, canonical.bytes());
        pos += canonical.len() + 2;

        if size == 0 {
            let mut trailers = TrailerBytes::default();
            loop {
                let line_end = find_crlf(stream, buf, pos, BodyLine::Trailer(trailers)).await?;
                let empty = line_end == pos;
                trailers.take(&buf[pos..line_end])?;
                pos = line_end + 2;
                if empty {
                    return Ok(pos);
                }
            }
        }

        decoded = decoded
            .checked_add(size)
            .filter(|total| limit.allows(*total))
            .ok_or(HttpInterceptError::Refused(RefusalError::BodyTooLarge))?;
        fill_to(stream, buf, pos + size + 2).await?;
        if &buf[pos + size..pos + size + 2] != b"\r\n" {
            return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
                HeadParseError::ExpectedLfAfterChunkData,
            )));
        }
        pos += size + 2;
    }
}

/// Streams the body that follows a request head from `client` (starting with
/// what `buf` already holds) to `upstream` as it arrives, keeping its framing.
/// Chunk sizes and the chunk terminators are checked on the way, so the
/// upstream never sees a body this proxy would not itself accept. Returns the
/// bytes sent. The body's size is not limited: nothing is held in memory.
/// Trailers are read and dropped, so the upstream never sees a field the
/// request rules did not.
pub(crate) async fn relay_request_body<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    framing: Framing,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    match framing {
        Framing::None => Ok(0),
        Framing::ContentLength(length) => relay_sized(client, buf, upstream, length).await,
        Framing::Chunked => {
            let mut sent = 0u64;
            loop {
                let line_end = find_crlf(client, buf, 0, BodyLine::ChunkSize).await?;
                let size = read_chunk_line(&buf[..line_end])?;
                let canonical = format!("{size:x}");
                buf.splice(..line_end, canonical.bytes());
                sent += forward_prefix(buf, upstream, canonical.len() + 2).await?;
                if size == 0 {
                    let mut trailers = TrailerBytes::default();
                    loop {
                        let line_end = find_crlf(client, buf, 0, BodyLine::Trailer(trailers)).await?;
                        let empty = line_end == 0;
                        trailers.take(&buf[..line_end])?;
                        if empty {
                            sent += forward_prefix(buf, upstream, 2).await?;
                            return Ok(sent);
                        }
                        buf.drain(..line_end + 2);
                    }
                }
                sent += relay_sized(client, buf, upstream, size).await?;
                fill_to(client, buf, 2).await?;
                if &buf[..2] != b"\r\n" {
                    return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
                        HeadParseError::ExpectedLfAfterChunkData,
                    )));
                }
                sent += forward_prefix(buf, upstream, 2).await?;
            }
        }
    }
}

async fn continue_if_owed<C>(client: &mut C, request: &ParsedRequest) -> Result<(), HttpInterceptError>
where
    C: AsyncWrite + Unpin,
{
    match request.pending {
        Some(pending) if expects_continue(&request.headers) && !pending.continued => {
            client
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .await
                .map_err(HttpInterceptError::ClientWrite)?;
            // The client holds its body until this reaches it.
            client.flush().await.map_err(HttpInterceptError::ClientWrite)
        }
        _ => Ok(()),
    }
}

/// Relays the body that is still on the client after `request`'s head went
/// upstream, scanning it for sentinels when the request was prepared for it.
async fn relay_body_only<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    request: &ParsedRequest,
    scope: Option<&SubstitutionScope>,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    let Some(pending) = request.pending else {
        return Ok(0);
    };
    let sent = match (pending.scan, scope) {
        (Some(outgoing), Some(scope)) if !has_opaque_content_encoding(&request.headers) => {
            relay_scanned_body(client, buf, upstream, pending.incoming, outgoing, scope).await
        }
        (Some(outgoing), _) => relay_rechunked_body(client, buf, upstream, pending.incoming, outgoing).await,
        _ => relay_request_body(client, buf, upstream, pending.incoming).await,
    }?;
    // A TLS upstream may keep the last record buffered when the socket was
    // not writable, and nothing else writes to it while the response is
    // awaited, so the upstream would wait on a body that never finishes.
    upstream.flush().await.map_err(HttpInterceptError::UpstreamWrite)?;
    Ok(sent)
}

/// What is left of a request's body on the client once its exchange is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientBody {
    /// Read to its end: the next request starts where it stopped.
    Read,
    /// Where it ends is not known, so the connection cannot carry another.
    Unread,
}

pub(crate) struct Exchanged {
    pub(crate) body_bytes: u64,
    pub(crate) relayed: Relayed,
    pub(crate) upstream: UpstreamAfter,
    pub(crate) client_body: ClientBody,
    /// When the response was all out: the tunnel is idle from then on.
    pub(crate) answered_at: tokio::time::Instant,
}

/// One response to `request`, relayed while the body still on the client is:
/// an upstream may answer before it has read the whole body, and the rest of
/// it is then read off the client and dropped.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn exchange<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    request: &ParsedRequest,
    scope: Option<&SubstitutionScope>,
    method: &str,
    scrub: Option<ResponseScrub<'_>>,
    hook: Option<&ResponseHook>,
    expects_continue: bool,
    answered_as: AnsweredRequest,
    stream_use: StreamUse,
) -> Result<Exchanged, HttpInterceptError>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    continue_if_owed(client, request).await?;
    let answered = AtomicBool::new(false);
    let (mut client_reads, mut client_writes) = tokio::io::split(&mut *client);
    let (mut upstream_reads, upstream_writes) = tokio::io::split(&mut *upstream);
    let mut body_sink = UntilAnswered::new(upstream_writes, &answered);
    let (body, response) = {
        let body = relay_body_only(&mut client_reads, buf, &mut body_sink, request, scope);
        let response = async {
            let relayed = relay_response_or_bad_gateway(
                &mut upstream_reads,
                &mut client_writes,
                method,
                scrub,
                hook,
                expects_continue,
                HopByHop::Stripped { client: answered_as },
                stream_use,
            )
            .await?;
            answered.store(true, Ordering::SeqCst);
            Ok::<_, HttpInterceptError>((relayed, tokio::time::Instant::now()))
        };
        tokio::pin!(body, response);
        tokio::select! {
            body_result = &mut body => match body_result {
                Err(HttpInterceptError::UpstreamWrite(error))
                    if error.kind() != std::io::ErrorKind::TimedOut =>
                {
                    (Err(HttpInterceptError::UpstreamWrite(error)), response.await?)
                }
                Err(error) => return Err(error),
                done => (done, response.await?),
            },
            response_result = &mut response => {
                let response_result = response_result?;
                (body.await, response_result)
            }
        }
    };
    let (relayed, answered_at) = response;
    let (body_bytes, client_body) = match body {
        Ok(bytes) => (bytes, ClientBody::Read),
        Err(HttpInterceptError::UpstreamWrite(_)) => (0, ClientBody::Unread),
        Err(error) => return Err(error),
    };
    let upstream = match (client_body, relayed.upstream) {
        (ClientBody::Read, Connection::KeepAlive) => body_sink.upstream_after(),
        _ => UpstreamAfter::Spent,
    };
    Ok(Exchanged {
        body_bytes,
        relayed,
        upstream,
        client_body,
        answered_at,
    })
}

/// Reads the rest of a request body that was answered before it came, and
/// drops it, so the client is not reset while it still sends. Gives up once
/// the client is quiet for [`DRAIN_QUIET`].
pub(crate) async fn drain_body<C>(client: &mut C, buf: &mut Vec<u8>, framing: Framing)
where
    C: AsyncRead + Unpin,
{
    drain_body_whole(client, buf, framing).await;
}

/// [`drain_body`], saying whether the body was read to its end.
pub(crate) async fn drain_body_whole<C>(client: &mut C, buf: &mut Vec<u8>, framing: Framing) -> bool
where
    C: AsyncRead + Unpin,
{
    let mut quiet = QuietReads::new(client);
    relay_request_body(&mut quiet, buf, &mut tokio::io::sink(), framing)
        .await
        .is_ok()
}

/// [`drain_body`] for a request that was answered as it stands.
pub(crate) async fn drain_pending_body<C>(client: &mut C, buf: &mut Vec<u8>, request: &ParsedRequest)
where
    C: AsyncRead + Unpin,
{
    match request.pending {
        Some(pending) if !expects_continue(&request.headers) || pending.continued => {
            drain_body(client, buf, pending.incoming).await;
        }
        _ => {}
    }
}

/// How a refused head says its body is framed, read loosely: enough to know
/// where it ends, when the head is not one this proxy would forward. No body
/// for a client that waits to be told to send it, and `None` when where it
/// ends cannot be told.
pub(crate) fn framing_of_refused_head(head: &[u8]) -> Option<Framing> {
    loose_framing(head, true)
}

/// [`framing_of_refused_head`] for a client that was told to continue, so
/// its body follows whatever the head says of it.
pub(crate) fn framing_after_continue(head: &[u8]) -> Option<Framing> {
    loose_framing(head, false)
}

fn loose_framing(head: &[u8], waits_for_continue: bool) -> Option<Framing> {
    let text = std::str::from_utf8(head).ok()?;
    let mut chunked = false;
    let mut lengths = Vec::new();
    for (name, value) in text.split("\r\n").skip(1).filter_map(|line| line.split_once(':')) {
        let value = value.trim();
        match name.trim() {
            name if waits_for_continue
                && name.eq_ignore_ascii_case("expect")
                && value.eq_ignore_ascii_case("100-continue") =>
            {
                return Some(Framing::None);
            }
            name if name.eq_ignore_ascii_case("transfer-encoding") => {
                chunked |= value.to_ascii_lowercase().contains("chunked");
            }
            name if name.eq_ignore_ascii_case("content-length") => {
                lengths.push(value.parse::<usize>().ok()?);
            }
            _ => {}
        }
    }
    match (chunked, lengths.as_slice()) {
        (true, _) => Some(Framing::Chunked),
        (false, []) => Some(Framing::None),
        (false, [first, rest @ ..]) if rest.iter().all(|length| length == first) => {
            Some(Framing::ContentLength(*first))
        }
        _ => None,
    }
}

/// [`relay_request_body`] passing the body through `scope`'s scanner. A
/// sentinel split across reads or chunks is still found, since the scanner
/// holds back the tail that could grow into one. `outgoing` is the framing
/// the upstream is told about: the body's own, or chunked when its length
/// can change.
pub(crate) async fn relay_scanned_body<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    incoming: Framing,
    outgoing: Framing,
    scope: &SubstitutionScope,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    relay_scanned(client, buf, upstream, incoming, outgoing, scope, Trailers::Forward).await
}

/// [`relay_scanned_body`] for a body the workload's HTTP stack would frame again, which carries no
/// trailers.
pub(crate) async fn relay_scanned_body_without_trailers<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    incoming: Framing,
    outgoing: Framing,
    scope: &SubstitutionScope,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    relay_scanned(client, buf, upstream, incoming, outgoing, scope, Trailers::Drop).await
}

async fn relay_scanned<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    incoming: Framing,
    outgoing: Framing,
    scope: &SubstitutionScope,
    trailers: Trailers,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    let mut scanner = scope.scanner();
    let relayed = scan_body(client, buf, upstream, incoming, outgoing, Some(&mut scanner), trailers).await;
    scope.report(&scanner.take_flagged()).await;
    relayed
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Trailers {
    Forward,
    Drop,
}

/// [`relay_scanned_body`] without a scanner: the body goes through as it is
/// and only its framing changes to `outgoing`.
pub(crate) async fn relay_rechunked_body<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    incoming: Framing,
    outgoing: Framing,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    scan_body(client, buf, upstream, incoming, outgoing, None, Trailers::Forward).await
}

/// [`relay_rechunked_body`] for a body the workload's HTTP stack would frame again, which carries
/// no trailers.
pub(crate) async fn relay_rechunked_body_without_trailers<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    incoming: Framing,
    outgoing: Framing,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    scan_body(client, buf, upstream, incoming, outgoing, None, Trailers::Drop).await
}

async fn scan_body<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    incoming: Framing,
    outgoing: Framing,
    mut scanner: Option<&mut Scanner<'_>>,
    keep: Trailers,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    let chunked_out = outgoing == Framing::Chunked;
    let mut sent = 0u64;
    let mut released = Vec::new();
    let mut trailers = Vec::new();
    match incoming {
        Framing::None => return Ok(0),
        Framing::ContentLength(length) => {
            let mut remaining = length;
            while remaining > 0 {
                let piece = take_piece(client, buf, remaining).await?;
                remaining -= piece.len();
                feed(scanner.as_deref_mut(), &piece, &mut released);
                sent += emit(upstream, &mut released, chunked_out).await?;
            }
        }
        Framing::Chunked => loop {
            let line_end = find_crlf(client, buf, 0, BodyLine::ChunkSize).await?;
            let size = read_chunk_line(&buf[..line_end])?;
            buf.drain(..line_end + 2);
            if size == 0 {
                let mut counted = TrailerBytes::default();
                loop {
                    let line_end = find_crlf(client, buf, 0, BodyLine::Trailer(counted)).await?;
                    let empty = line_end == 0;
                    counted.take(&buf[..line_end])?;
                    trailers.extend(buf.drain(..line_end + 2));
                    if empty {
                        break;
                    }
                }
                break;
            }
            let mut remaining = size;
            while remaining > 0 {
                let piece = take_piece(client, buf, remaining).await?;
                remaining -= piece.len();
                feed(scanner.as_deref_mut(), &piece, &mut released);
                sent += emit(upstream, &mut released, chunked_out).await?;
            }
            fill_to(client, buf, 2).await?;
            if &buf[..2] != b"\r\n" {
                return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
                    HeadParseError::ExpectedLfAfterChunkData,
                )));
            }
            buf.drain(..2);
        },
    }
    if let Some(scanner) = scanner {
        scanner.finish(&mut released);
    }
    sent += emit(upstream, &mut released, chunked_out).await?;
    if chunked_out {
        let last: &[u8] = if trailers.is_empty() || keep == Trailers::Drop {
            b"\r\n"
        } else {
            &trailers
        };
        upstream
            .write_all(b"0\r\n")
            .await
            .map_err(HttpInterceptError::UpstreamWrite)?;
        upstream
            .write_all(last)
            .await
            .map_err(HttpInterceptError::UpstreamWrite)?;
        sent += 3 + last.len() as u64;
    }
    Ok(sent)
}

fn feed(scanner: Option<&mut Scanner<'_>>, piece: &[u8], released: &mut Vec<u8>) {
    match scanner {
        Some(scanner) => scanner.feed(piece, released),
        None => released.extend_from_slice(piece),
    }
}

async fn take_piece<C>(client: &mut C, buf: &mut Vec<u8>, remaining: usize) -> Result<Vec<u8>, HttpInterceptError>
where
    C: AsyncRead + Unpin,
{
    if buf.is_empty() {
        fill_more(client, buf).await?;
    }
    let take = buf.len().min(remaining);
    Ok(buf.drain(..take).collect())
}

async fn emit<U>(upstream: &mut U, released: &mut Vec<u8>, chunked: bool) -> Result<u64, HttpInterceptError>
where
    U: AsyncWrite + Unpin,
{
    if released.is_empty() {
        return Ok(0);
    }
    let mut framed = Vec::with_capacity(released.len() + 16);
    if chunked {
        framed.extend_from_slice(format!("{:x}\r\n", released.len()).as_bytes());
        framed.extend_from_slice(released);
        framed.extend_from_slice(b"\r\n");
    } else {
        framed.extend_from_slice(released);
    }
    released.clear();
    upstream
        .write_all(&framed)
        .await
        .map_err(HttpInterceptError::UpstreamWrite)?;
    Ok(framed.len() as u64)
}

async fn forward_prefix<U>(buf: &mut Vec<u8>, upstream: &mut U, len: usize) -> Result<u64, HttpInterceptError>
where
    U: AsyncWrite + Unpin,
{
    upstream
        .write_all(&buf[..len])
        .await
        .map_err(HttpInterceptError::UpstreamWrite)?;
    buf.drain(..len);
    Ok(len as u64)
}

async fn relay_sized<C, U>(
    client: &mut C,
    buf: &mut Vec<u8>,
    upstream: &mut U,
    length: usize,
) -> Result<u64, HttpInterceptError>
where
    C: AsyncRead + Unpin,
    U: AsyncWrite + Unpin,
{
    let mut remaining = length;
    while remaining > 0 {
        if buf.is_empty() {
            fill_more(client, buf).await?;
        }
        let take = buf.len().min(remaining);
        forward_prefix(buf, upstream, take).await?;
        remaining -= take;
    }
    Ok(length as u64)
}

/// A line of a chunked request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyLine {
    ChunkSize,
    Trailer(TrailerBytes),
}

impl BodyLine {
    /// What a strict HTTP parser already refuses of the line `partial`, which has
    /// not yet ended.
    fn refusal_of(self, partial: &[u8]) -> Option<HeadParseError> {
        match self {
            Self::ChunkSize if !chunk_size_may_continue(partial) => Some(HeadParseError::InvalidChunkSize),
            Self::ChunkSize if extension_bytes(partial) > MAX_CHUNK_EXTENSION_BYTES => {
                Some(HeadParseError::ChunkExtensionsOverflow)
            }
            Self::Trailer(taken) if taken.holds(partial) => Some(HeadParseError::HeaderOverflow),
            _ if partial.len() > MAX_REQUEST_BODY_LINE_BYTES => Some(match self {
                Self::ChunkSize => HeadParseError::ChunkExtensionsOverflow,
                Self::Trailer(_) => HeadParseError::HeaderOverflow,
            }),
            _ => None,
        }
    }
}

/// The bytes a strict HTTP parser has counted of the trailers of a request so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TrailerBytes(usize);

impl TrailerBytes {
    fn of_line(line: &[u8]) -> usize {
        match line.iter().position(|&byte| byte == b':') {
            Some(colon) => {
                let value = &line[colon + 1..];
                let blanks = value.iter().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
                colon + value.len() - blanks
            }
            None => line.len(),
        }
    }

    fn holds(self, partial: &[u8]) -> bool {
        self.0 + Self::of_line(partial) >= MAX_REQUEST_TRAILER_BYTES
    }

    fn take(&mut self, line: &[u8]) -> Result<(), HttpInterceptError> {
        self.0 += Self::of_line(line);
        if self.0 >= MAX_REQUEST_TRAILER_BYTES {
            return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(
                HeadParseError::HeaderOverflow,
            )));
        }
        Ok(())
    }
}

fn extension_bytes(line: &[u8]) -> usize {
    line.iter()
        .position(|&byte| byte == b';')
        .map_or(0, |at| line.len() - at - 1)
}

/// Whether `extensions`, which start at the `;` of a chunk-size line, are
/// ones a strict HTTP parser reads: names, values and quoted values, with a `;`
/// never ending the line.
fn chunk_extensions_are_valid(extensions: &[u8]) -> bool {
    #[derive(Clone, Copy)]
    enum State {
        AfterSemicolon,
        Name,
        ValueStart,
        Value,
        Quoted,
        Escaped,
        AfterQuote,
    }
    let mut state = State::AfterSemicolon;
    for &byte in &extensions[1..] {
        let token = is_token_byte(byte);
        state = match (state, byte) {
            (State::AfterSemicolon | State::Name | State::Value | State::AfterQuote, b';')
            | (State::ValueStart, b';') => State::AfterSemicolon,
            (State::AfterSemicolon | State::Name, b'=') => State::ValueStart,
            (State::AfterSemicolon | State::Name, _) if token => State::Name,
            (State::ValueStart, b'"') => State::Quoted,
            (State::ValueStart | State::Value, _) if token => State::Value,
            (State::Quoted, b'"') => State::AfterQuote,
            (State::Quoted, b'\\') => State::Escaped,
            (State::Quoted, b'\t' | b' ' | 0x21 | 0x23..=0x5b | 0x5d..=0x7e | 0x80..=0xff) => State::Quoted,
            (State::Escaped, b'\t' | b' ' | 0x21..=0x7e | 0x80..=0xff) => State::Quoted,
            _ => return false,
        };
    }
    matches!(
        state,
        State::Name | State::ValueStart | State::Value | State::AfterQuote
    )
}

/// The size of the chunk a chunk-size line opens, once a strict HTTP parser accepts
/// the line.
fn read_chunk_line(line: &[u8]) -> Result<usize, HttpInterceptError> {
    let size = parse_chunk_size(line)?;
    if let Some(at) = line.iter().position(|&byte| byte == b';') {
        let error = if extension_bytes(line) > MAX_CHUNK_EXTENSION_BYTES {
            HeadParseError::ChunkExtensionsOverflow
        } else if !chunk_extensions_are_valid(&line[at..]) {
            HeadParseError::InvalidChunkExtensions
        } else {
            return Ok(size);
        };
        return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error)));
    }
    Ok(size)
}

/// The length of the message at the start of `body`, when all of it is there.
pub(crate) fn buffered_message_len(framing: Framing, body: &[u8]) -> Option<usize> {
    match framing {
        Framing::None => Some(0),
        Framing::ContentLength(length) => (body.len() >= length).then_some(length),
        Framing::Chunked => {
            let mut at = 0;
            loop {
                let line_len = body[at..].windows(2).position(|pair| pair == b"\r\n")?;
                let size = parse_chunk_size(&body[at..at + line_len]).ok()?;
                at += line_len + 2;
                if size == 0 {
                    loop {
                        let line_len = body[at..].windows(2).position(|pair| pair == b"\r\n")?;
                        at += line_len + 2;
                        if line_len == 0 {
                            return Some(at);
                        }
                    }
                }
                at = at.checked_add(size)?.checked_add(2)?;
                if at > body.len() {
                    return None;
                }
            }
        }
    }
}

/// What a strict HTTP parser says of `rest`, which follows a request that closes the
/// connection: anything but line breaks is data after `Connection: close`.
pub(crate) fn pipelined_error(keeps_alive: bool, rest: &[u8]) -> Option<HeadParseError> {
    (!keeps_alive && rest.iter().any(|byte| !matches!(byte, b'\r' | b'\n'))).then_some(HeadParseError::DataAfterClose)
}

async fn find_crlf<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    from: usize,
    line: BodyLine,
) -> Result<usize, HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    loop {
        if let Some(offset) = buf[from..].windows(2).position(|w| w == b"\r\n") {
            return Ok(from + offset);
        }
        if let Some(error) = line.refusal_of(&buf[from..]) {
            return Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error)));
        }
        fill_more(stream, buf).await?;
    }
}

/// Refuses a chunked body whose first bytes a strict HTTP parser already refuses,
/// reading once for them when none came with the head: a strict HTTP server hands
/// the request over only once its head is parsed, and a body refused that
/// early reaches no upstream.
pub(crate) async fn check_chunked_start<S>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    from: usize,
) -> Result<(), HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    if buf.len() == from {
        fill_more(stream, buf).await?;
    }
    match chunked_prefix_error(&buf[from..]) {
        Some(error) => Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error))),
        None => Ok(()),
    }
}

/// The error a strict HTTP parser raises on the chunks at the start of `body`, as
/// far as they have arrived.
fn chunked_prefix_error(body: &[u8]) -> Option<HeadParseError> {
    let mut at = 0;
    loop {
        let rest = &body[at..];
        let Some(line_len) = rest.windows(2).position(|pair| pair == b"\r\n") else {
            return BodyLine::ChunkSize.refusal_of(rest);
        };
        let size = match read_chunk_line(&rest[..line_len]) {
            Ok(size) => size,
            Err(HttpInterceptError::Refused(RefusalError::UnparsedBody(error))) => return Some(error),
            Err(_) => return None,
        };
        if size == 0 {
            return trailer_prefix_error(&rest[line_len + 2..]);
        }
        let data_end = (at + line_len + 2).saturating_add(size);
        let after = body.get(data_end..).unwrap_or_default();
        if !b"\r\n".starts_with(&after[..after.len().min(2)]) {
            return Some(HeadParseError::ExpectedLfAfterChunkData);
        }
        if after.len() < 2 {
            return None;
        }
        at = data_end + 2;
    }
}

/// The error a strict HTTP parser raises on the trailers at the start of `trailers`,
/// as far as they have arrived.
fn trailer_prefix_error(trailers: &[u8]) -> Option<HeadParseError> {
    let mut taken = TrailerBytes::default();
    let mut at = 0;
    loop {
        let rest = &trailers[at..];
        let Some(line_len) = rest.windows(2).position(|pair| pair == b"\r\n") else {
            return BodyLine::Trailer(taken).refusal_of(rest);
        };
        if line_len == 0 {
            return None;
        }
        if taken.take(&rest[..line_len]).is_err() {
            return Some(HeadParseError::HeaderOverflow);
        }
        at += line_len + 2;
    }
}

/// Whether `partial`, a chunk-size line still without its CRLF, can yet
/// become a valid one: a strict HTTP parser refuses the first byte that cannot.
fn chunk_size_may_continue(partial: &[u8]) -> bool {
    let size = partial.split(|b| *b == b';').next().unwrap_or_default();
    let size = size.strip_suffix(b"\r").unwrap_or(size);
    size.iter().all(u8::is_ascii_hexdigit) && !(size.is_empty() && partial.len() > size.len())
}

/// Reads from `stream` into `buf` until the `\r\n\r\n` header terminator is
/// found, returning the byte length of the header block (request line plus
/// headers, including the trailing blank line). Returns `None` if the
/// stream reaches EOF before any bytes arrive at all (a clean end of a
/// keep-alive connection); an EOF after partial bytes have arrived is a
/// malformed request.
async fn read_until_header_end<S>(stream: &mut S, buf: &mut Vec<u8>) -> Result<Option<usize>, HttpInterceptError>
where
    S: AsyncRead + Unpin,
{
    let mut chunk = vec![0u8; CLIENT_READ_BYTES];
    loop {
        let skipped = buf.iter().take_while(|byte| matches!(byte, b'\r' | b'\n')).count();
        buf.drain(..skipped);
        if let Some(refusal) = bare_line_feed_refusal(buf) {
            return Err(HttpInterceptError::Refused(refusal));
        }
        let awaiting_preface = matches!(Preface::of(buf), Some(Preface::Partial));
        if !awaiting_preface && let Some(pos) = find_request_head_end(buf) {
            if pos > REQUEST_HEAD_LIMIT {
                return Err(HttpInterceptError::Refused(RefusalError::Unparsed(
                    HeadParseError::HeaderOverflow,
                )));
            }
            return Ok(Some(pos));
        }
        if buf.len() > REQUEST_HEAD_LIMIT {
            return Err(HttpInterceptError::Refused(RefusalError::Unparsed(
                HeadParseError::HeaderOverflow,
            )));
        }

        let n = stream.read(&mut chunk).await.map_err(HttpInterceptError::ClientRead)?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(HttpInterceptError::MalformedRequest(
                "connection closed mid-request".into(),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// What a strict HTTP parser says of a line in the head that ends in a bare LF: one
/// is allowed only to end an HTTP/0.9 request line, and any other line it ends
/// is refused as soon as it arrives.
fn bare_line_feed_refusal(buf: &[u8]) -> Option<RefusalError> {
    let line_end = buf.iter().position(|&byte| byte == b'\n')?;
    if buf[..line_end].last() != Some(&b'\r') {
        let line = String::from_utf8_lossy(&buf[..line_end]);
        if let Err(error) = RequestLine::read(&line, LineEnd::BareLf) {
            return Some(RefusalError::unparsed(error));
        }
    }
    let mut start = line_end + 1;
    while let Some(offset) = buf[start..].iter().position(|&byte| byte == b'\n') {
        let end = start + offset;
        if end == start {
            return Some(RefusalError::Unparsed(HeadParseError::InvalidHeaderFieldChar));
        }
        if buf[end - 1] != b'\r' {
            return Some(RefusalError::Unparsed(HeadParseError::MissingCrAfterHeaderValue));
        }
        if end == start + 1 {
            return None;
        }
        start = end + 1;
    }
    None
}

/// The length of the request head in `buf`, whose request line may end in a
/// bare LF where it is an HTTP/0.9 one.
pub(crate) fn find_request_head_end(buf: &[u8]) -> Option<usize> {
    let line_end = buf.iter().position(|&byte| byte == b'\n')?;
    if line_end > 0 && buf[line_end - 1] != b'\r' && buf[line_end + 1..].starts_with(b"\r\n") {
        return Some(line_end + 3);
    }
    find_header_terminator(buf)
}

fn split_target(target: &str) -> (String, Vec<(String, String)>) {
    match target.split_once('?') {
        Some((path, query)) => (path.to_string(), parse_query_string(query)),
        None => (target.to_string(), Vec::new()),
    }
}

fn parse_query_string(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (key.to_string(), value.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

fn render_request(method: &str, target: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 256);
    out.extend_from_slice(format!("{method} {target} HTTP/1.1\r\n").as_bytes());
    for (name, value) in headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

fn target_with_query(path: &str, query: &[(String, String)]) -> String {
    if query.is_empty() {
        return path.to_string();
    }
    let query_string = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query_string}")
}

/// Rebuilds the request line + headers + body with the broker-issued token
/// injected per `injection`, overwriting any existing header/query value of
/// the same name. The token value itself is never logged: this function's
/// only output is the byte buffer handed directly to the upstream write.
fn inject(
    request: &ParsedRequest,
    injection: &CredentialInjection,
    token: &trogon_sandbox_contract::ShortLivedToken,
) -> Vec<u8> {
    let body = &request.raw[request.header_block_len..];

    let mut query = request.query.clone();
    let mut headers = request.headers.clone();

    match injection {
        CredentialInjection::Header(name) => {
            headers.retain(|(existing_name, _)| !existing_name.eq_ignore_ascii_case(name.as_str()));
            headers.push((name.as_str().to_string(), token.expose_secret().to_string()));
        }
        CredentialInjection::Query(key) => {
            query.retain(|(existing_key, _)| existing_key != key);
            query.push((key.clone(), token.expose_secret().to_string()));
        }
    }

    let target = target_with_query(&request.path, &query);
    render_request(&request.method, &target, &headers, body)
}

/// Original (SNI-sniffed) destination of a connection carrying a `Forward`
/// domain rule, so [`build_forwarded_request`] can tell the receiving proxy
/// at `forward_url` what the sandboxed workload actually asked for, since
/// `forward_url` itself replaces the connection's real destination.
pub(crate) struct OriginalDestination<'a> {
    pub(crate) host: &'a str,
    pub(crate) scheme: &'a str,
    pub(crate) port: u16,
}

/// Rebuilds a matched request's request line + headers + body with the
/// `trogon-forwarded-*` headers (see the constants above) plus
/// `trogon-sandbox-oidc-token` set to `oidc_token`'s encoded value, mirroring
/// [`inject`]'s "rewrite headers, keep body verbatim" shape. `oidc_token` is
/// only ever written into the header value here, never logged or otherwise
/// formatted through `{:?}`/`{}` elsewhere, keeping the invariant `OidcToken`'s
/// redacting `Debug` impl is meant to protect.
pub(crate) fn build_forwarded_request(
    request: &ParsedRequest,
    original_destination: &OriginalDestination<'_>,
    oidc_token: &OidcToken,
) -> Vec<u8> {
    let mut headers = request.headers.clone();
    for name in [
        FORWARDED_HOST_HEADER,
        FORWARDED_SCHEME_HEADER,
        FORWARDED_PORT_HEADER,
        FORWARDED_PATH_HEADER,
        SANDBOX_OIDC_TOKEN_HEADER,
    ] {
        headers.retain(|(existing_name, _)| !existing_name.eq_ignore_ascii_case(name));
    }
    headers.push((FORWARDED_HOST_HEADER.to_string(), original_destination.host.to_string()));
    headers.push((
        FORWARDED_SCHEME_HEADER.to_string(),
        original_destination.scheme.to_string(),
    ));
    headers.push((FORWARDED_PORT_HEADER.to_string(), original_destination.port.to_string()));
    headers.push((FORWARDED_PATH_HEADER.to_string(), request.path.clone()));
    headers.push((
        SANDBOX_OIDC_TOKEN_HEADER.to_string(),
        oidc_token.expose_secret().to_string(),
    ));

    let target = target_with_query(&request.path, &request.query);
    let body = &request.raw[request.header_block_len..];
    render_request(&request.method, &target, &headers, body)
}

/// Relays one HTTP/1.1 response from `upstream` to `client`. With a `scrub`
/// the response has real values swapped for their sentinels; either way its
/// framing is followed, so a keep-alive connection stays usable.
#[cfg(test)]
pub(crate) async fn relay_response<U, C>(
    upstream: &mut U,
    client: &mut C,
    method: &str,
    scrub: Option<ResponseScrub<'_>>,
    hook: Option<&ResponseHook>,
    expects_continue: bool,
) -> Result<Relayed, HttpInterceptError>
where
    U: AsyncRead + Unpin,
    C: AsyncWrite + Unpin,
{
    relay_response_hops(
        upstream,
        client,
        method,
        scrub,
        hook,
        expects_continue,
        HopByHop::Relayed,
    )
    .await
}

pub(crate) async fn relay_response_hops<U, C>(
    upstream: &mut U,
    client: &mut C,
    method: &str,
    scrub: Option<ResponseScrub<'_>>,
    hook: Option<&ResponseHook>,
    expects_continue: bool,
    hops: HopByHop,
) -> Result<Relayed, HttpInterceptError>
where
    U: AsyncRead + Unpin,
    C: AsyncWrite + Unpin,
{
    response_relay::relay_scrubbed(upstream, client, method, scrub.as_ref(), hook, expects_continue, hops).await
}

/// The 502 for a terminated tunnel at `at`, written by a strict HTTP server, which
/// keeps the client connection open after it unless the request asked to
/// close.
fn bad_gateway_bytes(request: AnsweredRequest, at: SystemTime) -> Vec<u8> {
    bad_gateway_answer().bytes(request, at)
}

pub(crate) fn bad_gateway_answer() -> FramedAnswer {
    FramedAnswer::plain(502, None, "Bad Gateway".to_string())
}

/// [`relay_response`] for a connection the client is still reading: a
/// failure before any byte reached the client is answered with a 502, and one
/// after the head went out ends the response by failing. A relayed response
/// is flushed to the client before it counts as sent, so a tunnel that ends after it does
/// not drop its tail.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn relay_response_or_bad_gateway<U, C>(
    upstream: &mut U,
    client: &mut C,
    method: &str,
    scrub: Option<ResponseScrub<'_>>,
    hook: Option<&ResponseHook>,
    expects_continue: bool,
    hops: HopByHop,
    stream_use: StreamUse,
) -> Result<Relayed, HttpInterceptError>
where
    U: AsyncRead + Unpin,
    C: AsyncWrite + Unpin,
{
    let mut watched = response_relay::WriteWatch::new(&mut *client);
    match relay_response_hops(upstream, &mut watched, method, scrub, hook, expects_continue, hops).await {
        Err(error) if !watched.wrote() && !error.ends_silently(stream_use) => {
            tracing::warn!(%error, "invalid or failed upstream response before its head was sent");
            let request = match hops {
                HopByHop::Stripped { client } => client,
                HopByHop::Relayed => AnsweredRequest::new(method, true),
            };
            let bad_gateway = bad_gateway_answer();
            let connection = if bad_gateway.keeps_alive(request) {
                Connection::KeepAlive
            } else {
                Connection::Close
            };
            let answer = bad_gateway.bytes(request, SystemTime::now());
            client
                .write_all(&answer)
                .await
                .map_err(HttpInterceptError::ClientWrite)?;
            let _ = client.flush().await;
            Ok(Relayed {
                bytes: answer.len() as u64,
                connection,
                upstream: Connection::Close,
                upstream_hint: crate::parent_pool::KeepAliveHint::none(),
            })
        }
        Ok(relayed) => {
            client.flush().await.map_err(HttpInterceptError::ClientWrite)?;
            Ok(relayed)
        }
        other => other,
    }
}

#[cfg(test)]
mod edge_tests;

#[cfg(test)]
mod target_tests;

#[cfg(test)]
mod tests;
