//! HTTP ingress: authenticated `CONNECT host:port` tunnels and absolute-form
//! `GET http://host/path` requests, parsed with bounded, strict framing.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::deny_explanation::standard_reason;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;
use trogon_sandbox_contract::DenyReason;
use trogon_sandbox_contract::EgressViolationReason;
use trogon_sandbox_contract::{Host, Port};
use trogon_sandbox_protocol::StandardDenyReason;

use super::token::ProxyToken;
use super::{ConnectTarget, IngressRejectionError, KEEP_ALIVE_TIMEOUT, deny_header};
use crate::framed_head::{AnsweredRequest, FramedAnswer, http_date};
use crate::head_parse_error::HeadParseError;
use crate::http_intercept::{
    CLIENT_READ_BYTES, drain_body, find_request_head_end, framing_after_continue, framing_of_refused_head,
};
use crate::refusal_error::RefusalError;
use crate::request_line::{LineEnd, PRI, Preface, RequestLine};
use crate::request_version::RequestVersion;
use crate::tls_ca::CRL_PATH;
use crate::violation_store::resolved_to_words;

mod constants;
use constants::{
    ALLOWLIST_DENY_BODY, BARE_LINE_FEED, MAX_HEADER_FIELDS, OPAQUE_TUNNEL_DENY_BODY, PLAIN_FRAMING, PROXY_REALM,
    TRUNCATED_HEAD,
};

/// A parsed, authenticated request. The CONNECT variant carries any bytes the
/// client sent after the head, which belong to the tunnel. The plain variant
/// carries the head as the client sent it and whatever followed it, which the
/// plain relay checks request by request.
#[derive(Debug)]
pub(super) enum HttpRequest {
    Connect {
        target: ConnectTarget,
        early_bytes: Vec<u8>,
    },
    Plain {
        head: Vec<u8>,
        leftover: Vec<u8>,
    },
    MalformedConnect {
        host: String,
        port: u16,
    },
    CertificateRevocationList,
}

#[derive(Debug)]
struct RequestHead {
    method: String,
    target: String,
    http11: bool,
    headers: Vec<(String, String)>,
}

impl RequestHead {
    fn headers_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn expects_continue(&self) -> bool {
        self.http11
            && self
                .headers_named("expect")
                .any(|value| value.trim().eq_ignore_ascii_case("100-continue"))
    }

    fn lacks_required_host(&self) -> bool {
        self.http11 && !self.method.eq_ignore_ascii_case("CONNECT") && self.headers_named("host").next().is_none()
    }
}

/// The clocks of one connection's first request: how long a head may take,
/// how long the connection may sit idle after a challenge that kept it open,
/// and the idle timeout the challenge advertises.
#[derive(Clone, Copy)]
pub(super) struct RequestTiming {
    pub head: Duration,
    pub idle_wait: Duration,
    pub advertised: Duration,
}

#[cfg(test)]
pub(super) async fn read_request<S>(
    stream: &mut S,
    token: Option<&ProxyToken>,
    max_head_bytes: usize,
    serves_crl: bool,
    ascii_hosts_only: bool,
    unmarked_challenge: bool,
) -> Result<HttpRequest, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    read_request_timed(
        stream,
        token,
        max_head_bytes,
        serves_crl.then_some(&[][..]),
        ascii_hosts_only,
        unmarked_challenge,
        None,
    )
    .await
}

async fn within<T>(limit: Option<Duration>, work: impl Future<Output = T>) -> Result<T, IngressRejectionError> {
    match limit {
        Some(limit) => timeout(limit, work)
            .await
            .map_err(|_| IngressRejectionError::HandshakeTimedOut),
        None => Ok(work.await),
    }
}

pub(super) async fn read_request_timed<S>(
    stream: &mut S,
    token: Option<&ProxyToken>,
    max_head_bytes: usize,
    crl: Option<&[u8]>,
    ascii_hosts_only: bool,
    unmarked_challenge: bool,
    timing: Option<RequestTiming>,
) -> Result<HttpRequest, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buffered = Vec::new();
    let mut challenged = false;
    let mut served_crl = false;
    let head_limit = timing.map(|timing| timing.head);
    let (head_bytes, head, leftover) = loop {
        let head_bytes = if challenged || served_crl {
            let ended = || {
                if served_crl {
                    Ok(HttpRequest::CertificateRevocationList)
                } else {
                    Err(IngressRejectionError::Unauthenticated)
                }
            };
            if buffered.is_empty() {
                let mut first = [0u8; 1];
                let idle = timing.map(|timing| timing.idle_wait);
                match within(idle, stream.read(&mut first)).await {
                    Ok(Ok(1)) => buffered.push(first[0]),
                    _ => return ended(),
                }
            }
            within(head_limit, read_next_head(stream, &mut buffered, max_head_bytes))
                .await??
                .unwrap_or_default()
        } else {
            match within(head_limit, read_head(stream, &mut buffered, max_head_bytes)).await? {
                Ok(parts) => parts,
                Err(rejection) => {
                    answer_head_error(stream, &rejection, max_head_bytes).await;
                    return Err(rejection);
                }
            }
        };

        let head = match parse_head(&head_bytes) {
            Ok(head) => head,
            Err(why) => {
                respond_bad_request(&mut Abrupt(&mut *stream)).await;
                return Err(IngressRejectionError::Malformed(why));
            }
        };

        if head.lacks_required_host() {
            respond_missing_host(stream, &head).await;
            return Err(IngressRejectionError::Malformed("missing Host header"));
        }

        if let Some(crl) = crl
            && head.method == "GET"
            && request_path(&head.target) == CRL_PATH
        {
            let framing = framing_of_refused_head(&head_bytes);
            let keep = framing.filter(|_| {
                head.http11
                    && !head
                        .headers_named("connection")
                        .any(|value| value.to_ascii_lowercase().contains("close"))
            });
            let advertised = timing.map_or(KEEP_ALIVE_TIMEOUT, |timing| timing.advertised);
            respond_crl(stream, crl, keep.is_some(), advertised).await;
            let Some(body) = keep else {
                return Ok(HttpRequest::CertificateRevocationList);
            };
            drain_body(stream, &mut buffered, body).await;
            served_crl = true;
            continue;
        }

        if is_authenticated(&head, token) {
            break (head_bytes, head, std::mem::take(&mut buffered));
        }
        let is_connect = head.method.eq_ignore_ascii_case("CONNECT");
        if is_connect || !unmarked_challenge {
            let framing = if is_connect { "" } else { PLAIN_FRAMING };
            respond_challenge(stream, unmarked_challenge, framing).await;
            return Err(IngressRejectionError::Unauthenticated);
        }
        let framing = framing_of_refused_head(&head_bytes);
        let keep = framing.filter(|_| {
            head.http11
                && !head
                    .headers_named("connection")
                    .any(|value| value.to_ascii_lowercase().contains("close"))
        });
        let advertised = timing.map_or(KEEP_ALIVE_TIMEOUT, |timing| timing.advertised);
        if head.expects_continue() {
            let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await;
            let _ = stream.flush().await;
        }
        respond_framed_challenge(stream, &head, keep.is_some(), advertised).await;
        let Some(framing) = keep else {
            return Err(IngressRejectionError::Unauthenticated);
        };
        let body = if head.expects_continue() {
            framing_after_continue(&head_bytes).unwrap_or(framing)
        } else {
            framing
        };
        drain_body(stream, &mut buffered, body).await;
        challenged = true;
    };

    if ascii_hosts_only && head.method.eq_ignore_ascii_case("CONNECT") && !is_ascii_authority(&head.target) {
        respond_bare(stream, 400, "Bad Request", deny_header::BAD_REQUEST).await;
        return Err(IngressRejectionError::Malformed("destination host is not plain ASCII"));
    }
    if !head.method.eq_ignore_ascii_case("CONNECT") {
        return Ok(HttpRequest::Plain {
            head: head_bytes,
            leftover,
        });
    }
    match parse_connect_target(&head.target) {
        Ok(ConnectAuthority::Target(target)) => Ok(HttpRequest::Connect {
            target,
            early_bytes: leftover,
        }),
        Ok(ConnectAuthority::Malformed { host, port }) => Ok(HttpRequest::MalformedConnect { host, port }),
        Err(why) => {
            respond_bare(stream, 400, "Bad Request", deny_header::BAD_REQUEST).await;
            Err(IngressRejectionError::Malformed(why))
        }
    }
}

/// The path of a request target in origin form or absolute form, without the
/// query or the fragment.
fn request_path(target: &str) -> &str {
    let rest = match target.find("://") {
        Some(at) if !target.starts_with('/') => {
            let after = &target[at + 3..];
            after.find(['/', '?', '#']).map_or("/", |i| &after[i..])
        }
        _ => target,
    };
    match rest.split(['?', '#']).next() {
        Some("") | None => "/",
        Some(path) => path,
    }
}

/// Answers a fetch of the empty revocation list. The client does not
/// authenticate: Windows fetches a certificate's distribution point itself.
async fn respond_crl<S: AsyncWrite + Unpin>(stream: &mut S, crl: &[u8], keeps_alive: bool, advertised: Duration) {
    let connection = if keeps_alive {
        format!("Connection: keep-alive\r\nKeep-Alive: timeout={}", advertised.as_secs())
    } else {
        "Connection: close".to_string()
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/pkix-crl\r\nContent-Length: {}\r\n{connection}\r\n\r\n",
        crl.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(crl).await;
    let _ = stream.flush().await;
    if !keeps_alive {
        let _ = stream.shutdown().await;
    }
}

/// Checks the proxy credentials of a later request on a connection whose
/// first request was already authenticated, answering 407 when they fail.
pub(super) async fn reauthenticate<S: AsyncWrite + Unpin>(
    stream: &mut S,
    head_bytes: &[u8],
    token: Option<&ProxyToken>,
    unmarked_challenge: bool,
) -> Result<(), IngressRejectionError> {
    match parse_head(head_bytes) {
        Err(why) => {
            respond_bare(stream, 400, "Bad Request", deny_header::BAD_REQUEST).await;
            Err(IngressRejectionError::Malformed(why))
        }
        Ok(head) if head.lacks_required_host() => {
            respond_missing_host(stream, &head).await;
            Err(IngressRejectionError::Malformed("missing Host header"))
        }
        Ok(head) if !is_authenticated(&head, token) => {
            respond_challenge(stream, unmarked_challenge, PLAIN_FRAMING).await;
            Err(IngressRejectionError::Unauthenticated)
        }
        Ok(_) => Ok(()),
    }
}

/// Reads the next request head of a connection that is already in use.
/// `None` is a clean end of the connection between two requests.
pub(super) async fn read_next_head<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    buffered: &mut Vec<u8>,
    max_head_bytes: usize,
) -> Result<Option<Vec<u8>>, IngressRejectionError> {
    if buffered.is_empty() {
        let mut first = [0u8; 1];
        if stream.read(&mut first).await? == 0 {
            return Ok(None);
        }
        buffered.push(first[0]);
    }
    match read_head(stream, buffered, max_head_bytes).await {
        Ok(head) => Ok(Some(head)),
        Err(rejection) => {
            answer_head_error(stream, &rejection, max_head_bytes).await;
            Err(rejection)
        }
    }
}

pub(super) async fn respond_established<S: AsyncWrite + Unpin>(stream: &mut S) -> std::io::Result<()> {
    stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    stream.flush().await
}

pub(super) async fn respond_blocked<S: AsyncWrite + Unpin>(stream: &mut S, phrase: &DenyPhrase) {
    respond_denied(stream, phrase.words(), deny_header::BLOCKED, ALLOWLIST_DENY_BODY).await;
}

/// The status phrase of a refused CONNECT.
pub(super) enum DenyPhrase {
    Forbidden,
    Reason(DenyReason),
}

impl DenyPhrase {
    /// The phrase for a refusal: by default a configured reason, else
    /// `Forbidden`; when `reasoned`, always the reason the proxy gives.
    pub(super) fn for_refusal(
        reasoned: bool,
        reason: &EgressViolationReason,
        explanation: Option<&DenyReason>,
    ) -> Self {
        if reasoned {
            let standard = match reason {
                EgressViolationReason::DeciderDenied => StandardDenyReason::UserDenied,
                _ => StandardDenyReason::NotOnAllowList,
            };
            return explanation
                .cloned()
                .or_else(|| standard_reason(standard))
                .map_or(Self::Forbidden, Self::Reason);
        }
        let on_deny_list = standard_reason(StandardDenyReason::OnDenyList);
        explanation
            .filter(|words| Some(*words) != on_deny_list.as_ref())
            .map_or(Self::Forbidden, |words| Self::Reason(words.clone()))
    }

    pub(super) fn malformed_host(reasoned: bool) -> Self {
        if reasoned {
            standard_reason(StandardDenyReason::MalformedHost).map_or(Self::Forbidden, Self::Reason)
        } else {
            Self::Forbidden
        }
    }

    fn words(&self) -> &str {
        match self {
            Self::Forbidden => "Forbidden",
            Self::Reason(reason) => reason.as_str(),
        }
    }
}

pub(super) async fn respond_address_denied<S: AsyncWrite + Unpin>(stream: &mut S, host: &Host, reason: &DenyReason) {
    let body = format!("Connection to {host} blocked: {}", reason.as_str());
    respond_denied(stream, reason.as_str(), deny_header::ADDRESS_DENIED, &body).await;
}

/// The words of a resolved-address refusal: the explanation the resolver
/// gave, or the one class when the refusal came without it.
pub(super) fn address_denial_words(
    reason: &EgressViolationReason,
    explanation: Option<&DenyReason>,
) -> Option<DenyReason> {
    let EgressViolationReason::DestinationAddressDenied(class) = reason else {
        return None;
    };
    explanation
        .cloned()
        .or_else(|| DenyReason::new(&resolved_to_words(&[*class])).ok())
}

/// The answer to a plain-HTTP request whose name resolved to a refused
/// address, as the proxy writes it
///: `403 Forbidden`, with the host and the
/// reason as the body.
pub(super) fn address_denied_plain_answer(host: &Host, reason: &DenyReason) -> FramedAnswer {
    FramedAnswer::plain(
        403,
        crate::deny_header_style::header_field(deny_header::ADDRESS_DENIED),
        format!("Connection to {host} blocked: {}\n", reason.as_str()),
    )
}

pub(super) async fn respond_opaque_refused<S: AsyncWrite + Unpin>(stream: &mut S) {
    respond_denied(stream, "Forbidden", deny_header::OPAQUE_TUNNEL, OPAQUE_TUNNEL_DENY_BODY).await;
}

/// The answer to a plain-HTTP request for a refused destination, which the proxy
/// writes through the workload's HTTP stack.
pub(super) fn blocked_plain_answer() -> FramedAnswer {
    FramedAnswer::plain(
        403,
        crate::deny_header_style::header_field(deny_header::BLOCKED),
        format!("{ALLOWLIST_DENY_BODY}\n"),
    )
}

/// A policy refusal on a CONNECT: `text/plain`, the reason as the status
/// phrase, the explanation as a body that the end of the connection delimits.
async fn respond_denied<S: AsyncWrite + Unpin>(stream: &mut S, phrase: &str, error: &str, body: &str) {
    let phrase: String = phrase
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect();
    let out = format!(
        "HTTP/1.1 403 {phrase}\r\nContent-Type: text/plain\r\n{}\r\n{body}",
        crate::deny_header_style::header_line(error)
    );
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

pub(super) async fn respond_unreachable<S: AsyncWrite + Unpin>(stream: &mut S) {
    respond_bare(stream, 502, "Bad Gateway", deny_header::UNREACHABLE).await;
}

/// The refusal at a configured tunnel cap, shaped as the proxy
/// shapes it: no length, and the end of the connection delimits the body.
pub(super) async fn respond_tunnel_cap<S: AsyncWrite + Unpin>(stream: &mut S) {
    let out = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\n{}: {}\r\nConnection: close\r\n\r\ntoo many TLS-terminated tunnels at once",
        deny_header::NAME,
        deny_header::TOO_MANY_TUNNELS
    );
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// A writer whose shutdown is a no-op, so the answer is followed by the
/// close alone and a client with unsent bytes behind it sees a reset, as it
/// does when the workload's HTTP stack destroys the socket.
pub(super) struct Abrupt<'a, S>(pub(super) &'a mut S);

impl<S: AsyncWrite + Unpin> AsyncWrite for Abrupt<'_, S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub(super) async fn respond_bad_request<S: AsyncWrite + Unpin>(stream: &mut S) {
    respond_bare(stream, 400, "Bad Request", deny_header::BAD_REQUEST).await;
}

/// The bare answer the workload's HTTP stack writes on a plain connection for a body its
/// parser refused, which ends the request before the proxy answers it.
pub(super) async fn respond_parse_error<S: AsyncWrite + Unpin>(stream: &mut S, error: HeadParseError) {
    match error {
        HeadParseError::HeaderOverflow => respond_head_too_large(stream, super::MAX_HEAD_BYTES).await,
        _ => respond_bad_request(stream).await,
    }
}

/// The 407 that asks for the proxy credentials. The proxy sends only the challenge,
/// so `unmarked` leaves out the mark naming the refusal. The connection is
/// closed after it.
/// The 407 the workload's HTTP stack writes for a plain request: chunked and
/// kept alive for an HTTP/1.1 client that allows it, closing otherwise.
async fn respond_framed_challenge<S: AsyncWrite + Unpin>(
    stream: &mut S,
    head: &RequestHead,
    keeps_alive: bool,
    advertised: Duration,
) {
    let date = http_date(std::time::SystemTime::now());
    let tail = if keeps_alive {
        format!(
            "Connection: keep-alive\r\nKeep-Alive: timeout={}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            advertised.as_secs()
        )
    } else if head.http11 {
        "Connection: close\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".to_string()
    } else {
        "Connection: close\r\n\r\n".to_string()
    };
    let out = format!(
        "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"{PROXY_REALM}\"\r\nDate: {date}\r\n{tail}"
    );
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.flush().await;
    if !keeps_alive {
        let _ = stream.shutdown().await;
    }
}

async fn respond_challenge<S: AsyncWrite + Unpin>(stream: &mut S, unmarked: bool, framing: &str) {
    let challenge = format!("Proxy-Authenticate: Basic realm=\"{PROXY_REALM}\"\r\n");
    let mark = if unmarked {
        String::new()
    } else {
        crate::deny_header_style::header_line(deny_header::UNAUTHENTICATED)
    };
    let out = format!("HTTP/1.1 407 Proxy Authentication Required\r\n{mark}{challenge}{framing}\r\n");
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// A refusal of a request the proxy writes straight to the socket
/// (a CONNECT, a head its HTTP parser refused): only the status line, the
/// mark and the listed headers, with the end of the connection ending it.
async fn respond_bare<S: AsyncWrite + Unpin>(stream: &mut S, status: u16, reason: &str, error: &str) {
    write_refusal(stream, status, reason, error).await;
}

async fn answer_head_error<S: AsyncWrite + Unpin>(
    stream: &mut S,
    rejection: &IngressRejectionError,
    max_head_bytes: usize,
) {
    let stream = &mut Abrupt(stream);
    match rejection {
        IngressRejectionError::HeadTooLarge => respond_head_too_large(stream, max_head_bytes).await,
        IngressRejectionError::Malformed(why) if *why == BARE_LINE_FEED || *why == TRUNCATED_HEAD => {
            respond_bare(stream, 400, "Bad Request", deny_header::BAD_REQUEST).await;
        }
        _ => {}
    }
}

/// The answer of the workload's HTTP stack itself to an HTTP/1.1 request with no
/// `Host` header, which comes before any of the proxy's handlers.
async fn respond_missing_host<S: AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead) {
    let out = RefusalError::MissingHost.answer(AnsweredRequest::new(&head.method, true), std::time::SystemTime::now());
    let _ = stream.write_all(&out).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

async fn respond_head_too_large<S: AsyncWrite + Unpin>(stream: &mut S, max_head_bytes: usize) {
    if max_head_bytes == super::MAX_HEAD_BYTES {
        respond_bare(stream, 431, "Request Header Fields Too Large", deny_header::BAD_REQUEST).await;
    } else {
        respond_bare(stream, 400, "Bad Request", deny_header::BAD_REQUEST).await;
    }
}

async fn write_refusal<S: AsyncWrite + Unpin>(stream: &mut S, status: u16, reason: &str, error: &str) {
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\n{}",
        crate::deny_header_style::header_line(error)
    );
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// Reads one request head the way the reference parser does: leading line
/// breaks are skipped, a line feed without its carriage return ends the
/// request, and the limit is on the bytes of the request target, header names
/// and header values, not on the head as written.
async fn read_head<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    max_head_bytes: usize,
) -> Result<Vec<u8>, IngressRejectionError> {
    let mut chunk = vec![0u8; CLIENT_READ_BYTES];
    loop {
        let skipped = buf.iter().take_while(|b| matches!(b, b'\r' | b'\n')).count();
        buf.drain(..skipped);
        if has_bare_line_feed(buf) {
            return Err(IngressRejectionError::Malformed(BARE_LINE_FEED));
        }
        let awaiting_preface = matches!(Preface::of(buf), Some(Preface::Partial));
        if !awaiting_preface && let Some(end) = find_request_head_end(buf) {
            if counted_bytes(&buf[..end]) >= max_head_bytes || header_fields(&buf[..end]) > MAX_HEADER_FIELDS {
                return Err(IngressRejectionError::HeadTooLarge);
            }
            let leftover = buf.split_off(end);
            return Ok(std::mem::replace(buf, leftover));
        }
        if counted_bytes(buf) >= max_head_bytes
            || header_fields(buf) > MAX_HEADER_FIELDS
            || buf.len() > max_head_bytes * 4 + 1024
        {
            return Err(IngressRejectionError::HeadTooLarge);
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(IngressRejectionError::Malformed(
                if buf.iter().all(|b| matches!(b, b'\r' | b'\n')) {
                    "connection closed before a head"
                } else {
                    TRUNCATED_HEAD
                },
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Whether a line of the head ends in a line feed without its carriage
/// return, which the reference parser allows only at the end of an HTTP/0.9 request line.
fn has_bare_line_feed(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().any(|(at, &b)| {
        b == b'\n'
            && (at == 0 || bytes[at - 1] != b'\r')
            && !(bytes[..at].iter().all(|&b| b != b'\n')
                && RequestLine::read(&String::from_utf8_lossy(&bytes[..at]), LineEnd::BareLf).is_ok())
    })
}

/// The header lines of a head, which the reference parser refuses past a thousand with the
/// same overflow error as a head past its size.
fn header_fields(head: &[u8]) -> usize {
    head.split(|&b| b == b'\n')
        .skip(1)
        .filter(|line| !line.strip_suffix(b"\r").unwrap_or(line).is_empty())
        .count()
}

/// What the reference parser measures against `maxHeaderSize`: the bytes of the
/// request target and of every header name and value, trailing blanks of a
/// value included.
fn counted_bytes(head: &[u8]) -> usize {
    let mut total = 0;
    for (index, line) in head.split(|&b| b == b'\n').enumerate() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            break;
        }
        if index == 0 {
            total += line.split(|&b| b == b' ').nth(1).map_or(0, <[u8]>::len);
        } else if let Some(colon) = line.iter().position(|&b| b == b':') {
            let value = &line[colon + 1..];
            let blanks = value.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
            total += colon + value.len() - blanks;
        } else {
            total += line.len();
        }
    }
    total
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn parse_head(bytes: &[u8]) -> Result<RequestHead, &'static str> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => std::borrow::Cow::Borrowed(text),
        Err(_) => {
            let lossy = String::from_utf8_lossy(bytes);
            if lossy
                .get(.."CONNECT ".len())
                .is_none_or(|method| method.eq_ignore_ascii_case("CONNECT "))
            {
                return Err("head is not valid UTF-8");
            }
            lossy
        }
    };
    let text = text
        .strip_suffix("\r\n\r\n")
        .or_else(|| text.strip_suffix("\n\r\n"))
        .ok_or("head is not terminated")?;
    let (request_line, end, rest) = RequestLine::split(text);
    if request_line.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err("control character in request line");
    }
    let RequestLine {
        method,
        target,
        version,
        ..
    } = RequestLine::read(request_line, end).map_err(|_| "malformed request line")?;
    if method == PRI {
        return Err("malformed request line");
    }
    let lines = rest.split("\r\n").filter(|line| !line.is_empty());

    let mut headers = Vec::new();
    let mut framing = Framing::default();
    for line in lines {
        if line.starts_with([' ', '\t']) {
            return Err("obsolete line folding");
        }
        let (name, value) = line.split_once(':').ok_or("malformed header line")?;
        if name.is_empty() || !name.bytes().all(is_token_char) {
            return Err("malformed header name");
        }
        if value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
            return Err("control character in header value");
        }
        framing.note(name, value.trim_matches([' ', '\t']))?;
        headers.push((name.to_string(), value.trim_matches([' ', '\t']).to_string()));
    }

    framing.finish()?;

    Ok(RequestHead {
        method: method.to_string(),
        target: target.to_string(),
        http11: version == RequestVersion::Http11,
        headers,
    })
}

/// The body framing rules of the reference parser: one `Content-Length` of digits
/// that fit 64 bits, no `Transfer-Encoding` beside it, and `chunked` only as
/// the last coding of the last `Transfer-Encoding` header.
#[derive(Default)]
struct Framing {
    content_length: bool,
    transfer_encoding: bool,
    chunked: bool,
}

impl Framing {
    fn note(&mut self, name: &str, value: &str) -> Result<(), &'static str> {
        if name.eq_ignore_ascii_case("content-length") {
            if std::mem::replace(&mut self.content_length, true) {
                return Err("duplicate Content-Length");
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) || value.parse::<u64>().is_err() {
                return Err("invalid Content-Length");
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            self.transfer_encoding = true;
            if value.is_empty() || self.chunked {
                return Err("invalid Transfer-Encoding");
            }
            let mut codings = value.split(',').map(|coding| coding.trim_matches([' ', '\t']));
            let last = codings.next_back();
            if codings.any(|coding| coding.eq_ignore_ascii_case("chunked")) {
                return Err("invalid Transfer-Encoding");
            }
            self.chunked = last.is_some_and(|coding| coding.eq_ignore_ascii_case("chunked"));
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), &'static str> {
        if self.content_length && self.transfer_encoding {
            return Err("Content-Length with Transfer-Encoding");
        }
        Ok(())
    }
}

fn is_authenticated(head: &RequestHead, token: Option<&ProxyToken>) -> bool {
    let Some(token) = token else {
        return true;
    };
    let mut values = head.headers_named("proxy-authorization");
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    let Some((scheme, credentials)) = value.split_once(' ') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(decoded) = STANDARD.decode(credentials.trim()) else {
        return false;
    };
    let Some(colon) = decoded.iter().position(|&b| b == b':').filter(|&at| at > 0) else {
        return false;
    };
    token.verify(&decoded[colon + 1..])
}

enum ConnectAuthority {
    Target(ConnectTarget),
    Malformed { host: String, port: u16 },
}

/// A CONNECT target the way the proxy splits it: `[host]:digits` or
/// `host:digits` with no colon in the host, a port of 1 to 65535 whatever
/// zeros lead it. A host that is not a plain name or address is not a bad
/// request: it reaches the allow list check, which denies it as malformed.
fn parse_connect_target(input: &str) -> Result<ConnectAuthority, &'static str> {
    let (host, port) = match input.strip_prefix('[') {
        Some(rest) => rest
            .split_once("]:")
            .filter(|(host, _)| !host.is_empty() && !host.contains(']')),
        None => None,
    }
    .or_else(|| {
        input
            .split_once(':')
            .filter(|(host, port)| !host.is_empty() && !port.contains(':'))
    })
    .ok_or("malformed authority")?;
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err("malformed authority");
    }
    let port: u16 = port
        .trim_start_matches('0')
        .parse()
        .ok()
        .filter(|port| *port != 0)
        .ok_or("invalid port")?;
    let bracketed = input.starts_with('[');
    let spelled = if bracketed {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let target = super::socks5::is_valid_host(&spelled)
        .then(|| Host::new(&spelled).ok())
        .flatten()
        .zip(Port::new(port).ok());
    Ok(match target {
        Some((host, port)) => ConnectAuthority::Target(ConnectTarget::new(host, port)),
        None => ConnectAuthority::Malformed {
            host: host.to_string(),
            port,
        },
    })
}

/// Whether the destination host of an authority is an IP literal or letters,
/// digits and `-` in non-empty dot-separated labels with one trailing dot at
/// most.
pub(super) fn is_ascii_authority(authority: &str) -> bool {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest
            .split_once(']')
            .is_some_and(|(inner, _)| inner.parse::<std::net::Ipv6Addr>().is_ok());
    }
    let host = authority.split(':').next().unwrap_or_default();
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    !name.is_empty()
        && name
            .split('.')
            .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// `host:port` or `[v6]:port`. A missing port is an error unless
/// `default_port` supplies one.
pub(super) fn parse_authority(input: &str, default_port: Option<u16>) -> Result<ConnectTarget, &'static str> {
    if input.contains(['@', '/', '?', '#', '\\', ' ']) {
        return Err("malformed authority");
    }
    let (host_text, port_text) = if input.starts_with('[') {
        let end = input.find(']').ok_or("malformed authority")?;
        let rest = &input[end + 1..];
        let port = match rest {
            "" => None,
            _ => Some(rest.strip_prefix(':').ok_or("malformed authority")?),
        };
        (&input[..=end], port)
    } else {
        match input.split_once(':') {
            Some((_, port)) if port.contains(':') => return Err("malformed authority"),
            Some((host, port)) => (host, Some(port)),
            None => (input, None),
        }
    };
    let host = Host::new(host_text).map_err(|_| "invalid host")?;
    let port = match (port_text, default_port) {
        (Some(text), _) => Port::from_str(text).map_err(|_| "invalid port")?,
        (None, Some(default)) => Port::new(default).map_err(|_| "invalid port")?,
        (None, None) => return Err("CONNECT target needs a port"),
    };
    Ok(ConnectTarget::new(host, port))
}

#[cfg(test)]
mod tests;
