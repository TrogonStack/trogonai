//! Response relay that swaps real secret values for their sentinels, so an
//! allowed host that reflects a substituted request cannot return a secret
//! into the sandbox.
//!
//! The status line, headers, trailers and uncompressed body are scanned as a
//! stream with the scope's real to sentinel automaton, holding back the last
//! `longest real value - 1` bytes of each piece so a value split across reads
//! or HTTP chunks is still seen whole. Memory stays bounded: a body is only
//! buffered when its `Content-Length` fits the [`BodyLimit`], because scrubbing
//! can change its length and the header must say so before the first body
//! byte; a larger one is streamed as chunked (or close-delimited for an
//! HTTP/1.0 upstream). A compressed body cannot be scanned and is handled by
//! the scope's [`CompressedResponsePolicy`].
//!
//! Framing follows RFC 9112: HEAD, 204 and 304 responses have no body,
//! interim 1xx responses are relayed and skipped, and a body without a length
//! runs to the end of the connection, which ends it.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::SystemTime;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::body_limit::BodyLimit;
use crate::chunked_body::parse_chunk_size;
use crate::constants::{MAX_CHUNK_LINE_BYTES, MAX_TRAILER_BYTES, READ_CHUNK_BYTES};
use crate::framed_head::{AnsweredRequest, FramedAnswer, FramedHead, parsed_fields};
use crate::http_intercept_error::HttpInterceptError;
use crate::parent_pool::KeepAliveHint;
use crate::request_decision::ResponseHook;
use crate::request_version::RequestVersion;
use crate::response_head::{has_opaque_content_encoding, parse_response_headers, read_response_header_end};
use crate::response_strip::strip_response_headers;
use crate::secret_substitution::{CompressedResponsePolicy, ResponseRules, Scanner, SubstitutionScope};
use crate::status_code::StatusCode;
use trogon_sandbox_contract::ResponseHeaderStrip;

mod constants;
use constants::HOP_BY_HOP_FIELDS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Connection {
    KeepAlive,
    Close,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Relayed {
    pub(crate) bytes: u64,
    /// What the client's connection does next.
    pub(crate) connection: Connection,
    /// What the upstream's connection does next: it cannot carry another
    /// request when the response said `Connection: close` or ran to the end
    /// of the connection.
    pub(crate) upstream: Connection,
    /// How long the upstream's connection may sit idle, by its response.
    pub(crate) upstream_hint: KeepAliveHint,
}

/// What a response head says of the connections it crosses. A terminated
/// tunnel answers: the upstream response's hop-by-hop fields are
/// dropped and the client's
/// connection is framed anew by the workload's HTTP stack,
/// so an upstream that closes its connection does not end the tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HopByHop {
    Relayed,
    Stripped { client: AnsweredRequest },
}

impl HopByHop {
    fn is_stripped(self) -> bool {
        matches!(self, Self::Stripped { .. })
    }

    fn client(self) -> Option<AnsweredRequest> {
        match self {
            Self::Relayed => None,
            Self::Stripped { client } => Some(client),
        }
    }
}

/// `head` without the fields the proxy drops: the hop-by-hop set
/// and every field the `Connection` field names. The status line is spoken
/// as HTTP/1.1, which is what the workload's HTTP stack writes whatever the upstream
/// spoke.
fn strip_hop_by_hop(head: &str) -> String {
    let block = head.strip_suffix("\r\n\r\n").unwrap_or(head);
    let mut lines = block.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let fields: Vec<&str> = lines.collect();
    let named: Vec<String> = fields
        .iter()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let mut kept = vec![match status_line.strip_prefix("HTTP/1.0") {
        Some(rest) => format!("HTTP/1.1{rest}"),
        None => status_line.to_string(),
    }];
    let mut dropping = false;
    for line in fields {
        if line.starts_with([' ', '\t']) {
            if !dropping {
                kept.push(line.to_string());
            }
            continue;
        }
        let name = line
            .split_once(':')
            .map_or(line, |(name, _)| name)
            .trim()
            .to_ascii_lowercase();
        dropping = HOP_BY_HOP_FIELDS.contains(&name.as_str()) || named.contains(&name);
        if !dropping {
            kept.push(line.to_string());
        }
    }
    let mut stripped = kept.join("\r\n");
    stripped.push_str("\r\n\r\n");
    stripped
}

/// The head the workload's HTTP stack writes for the upstream's `head` once its
/// hop-by-hop fields are gone: the fields as `IncomingMessage.headers` holds
/// them, given to `writeHead` with the upstream's status
///.
fn framed_response_head(head: &str, status: u16, edit: Option<HeadEdit>) -> FramedHead {
    let mut fields = parsed_fields(&parse_response_headers(head));
    match edit {
        Some(HeadEdit::SetLength(length)) => fields
            .iter_mut()
            .filter(|(name, _)| name == "content-length")
            .for_each(|(_, value)| *value = length.to_string()),
        Some(HeadEdit::DropLength | HeadEdit::DropLengthAndChunk) => {
            fields.retain(|(name, _)| name != "content-length");
        }
        None => {}
    }
    FramedHead::new(status, fields)
}

/// Whether a head continues a field on a line of its own (obs-fold), which
/// the reference parser refuses in a response.
fn has_folded_line(head: &str) -> bool {
    head.split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .any(|line| line.starts_with([' ', '\t']))
}

#[derive(Clone, Copy)]
pub(crate) struct ResponseScrub<'a> {
    scope: Option<&'a SubstitutionScope>,
    rules: Option<ResponseRules<'a>>,
    network_strip: Option<&'a ResponseHeaderStrip>,
    limit: BodyLimit,
}

impl<'a> ResponseScrub<'a> {
    pub(crate) fn new(
        scope: Option<&'a SubstitutionScope>,
        network_strip: Option<&'a ResponseHeaderStrip>,
        limit: BodyLimit,
    ) -> Option<Self> {
        let rules = scope.and_then(SubstitutionScope::response_rules);
        let network_strip = network_strip.filter(|strip| !strip.is_empty());
        let scope_strips = scope.is_some_and(|scope| !scope.response_strip().is_empty());
        if rules.is_none() && !scope_strips && network_strip.is_none() {
            return None;
        }
        Some(Self {
            scope,
            rules,
            network_strip,
            limit,
        })
    }

    pub(crate) fn network_only(network_strip: &'a ResponseHeaderStrip, limit: BodyLimit) -> Option<Self> {
        Self::new(None, Some(network_strip), limit)
    }

    fn strip(&self, head: String) -> String {
        let head = match self.scope {
            Some(scope) => strip_response_headers(&head, scope.response_strip()),
            None => head,
        };
        match self.network_strip {
            Some(strip) => strip_response_headers(&head, strip),
            None => head,
        }
    }
}

enum Filter<'a> {
    Scrub(Scanner<'a>),
    Verbatim,
}

impl<'a> Filter<'a> {
    fn new(rules: Option<ResponseRules<'a>>, scrubbing: bool) -> Self {
        match rules {
            Some(rules) if scrubbing => Self::Scrub(rules.scanner()),
            _ => Self::Verbatim,
        }
    }

    fn feed(&mut self, piece: &[u8]) -> Vec<u8> {
        match self {
            Self::Scrub(scanner) => {
                let mut out = Vec::with_capacity(piece.len());
                scanner.feed(piece, &mut out);
                out
            }
            Self::Verbatim => piece.to_vec(),
        }
    }

    fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Self::Scrub(scanner) = self {
            scanner.finish(&mut out);
        }
        out
    }

    fn scrubbed(&self) -> usize {
        match self {
            Self::Scrub(scanner) => scanner.replaced(),
            Self::Verbatim => 0,
        }
    }
}

enum Framing {
    Empty,
    Sized(usize),
    Chunked,
    UntilClose,
}

enum HeadEdit {
    SetLength(usize),
    DropLength,
    DropLengthAndChunk,
}

struct UpstreamReader<'a, U> {
    stream: &'a mut U,
    buf: Vec<u8>,
}

impl<U: AsyncRead + Unpin> UpstreamReader<'_, U> {
    async fn fill(&mut self) -> Result<usize, HttpInterceptError> {
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        let n = self
            .stream
            .read(&mut chunk)
            .await
            .map_err(HttpInterceptError::UpstreamRead)?;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n)
    }

    async fn line(&mut self, max: usize) -> Result<Vec<u8>, HttpInterceptError> {
        loop {
            if let Some(end) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = self.buf.drain(..end).collect();
                self.buf.drain(..2);
                return Ok(line);
            }
            if self.buf.len() > max {
                return Err(malformed("response line too long"));
            }
            if self.fill().await? == 0 {
                return Err(malformed("upstream closed mid-body"));
            }
        }
    }

    async fn piece(&mut self, remaining: usize) -> Result<Vec<u8>, HttpInterceptError> {
        if self.buf.is_empty() && self.fill().await? == 0 {
            return Err(malformed("upstream closed mid-body"));
        }
        let take = self.buf.len().min(remaining).min(READ_CHUNK_BYTES);
        Ok(self.buf.drain(..take).collect())
    }

    async fn until_close(&mut self) -> Result<Option<Vec<u8>>, HttpInterceptError> {
        if self.buf.is_empty() && self.fill().await? == 0 {
            return Ok(None);
        }
        let take = self.buf.len().min(READ_CHUNK_BYTES);
        Ok(Some(self.buf.drain(..take).collect()))
    }

    async fn trailers(&mut self) -> Result<Vec<u8>, HttpInterceptError> {
        let mut trailers = Vec::new();
        loop {
            let line = self.line(MAX_CHUNK_LINE_BYTES).await?;
            if line.is_empty() {
                return Ok(trailers);
            }
            trailers.extend_from_slice(&line);
            trailers.extend_from_slice(b"\r\n");
            if trailers.len() > MAX_TRAILER_BYTES {
                return Err(malformed("response trailers too large"));
            }
        }
    }
}

struct ClientWriter<'a, C> {
    stream: &'a mut C,
    written: u64,
}

impl<C: AsyncWrite + Unpin> ClientWriter<'_, C> {
    async fn send(&mut self, bytes: &[u8]) -> Result<(), HttpInterceptError> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.stream
            .write_all(bytes)
            .await
            .map_err(HttpInterceptError::ClientWrite)?;
        // The upstream is read next and may idle (an interim head, a stream),
        // so a TLS record left buffered here would never reach the client.
        self.stream.flush().await.map_err(HttpInterceptError::ClientWrite)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    async fn chunk(&mut self, payload: &[u8]) -> Result<(), HttpInterceptError> {
        if payload.is_empty() {
            return Ok(());
        }
        let mut framed = format!("{:x}\r\n", payload.len()).into_bytes();
        framed.extend_from_slice(payload);
        framed.extend_from_slice(b"\r\n");
        self.send(&framed).await
    }
}

fn malformed(reason: &str) -> HttpInterceptError {
    HttpInterceptError::MalformedRequest(reason.into())
}

pub(crate) async fn relay_scrubbed<U, C>(
    upstream: &mut U,
    client: &mut C,
    method: &str,
    scrub: Option<&ResponseScrub<'_>>,
    hook: Option<&ResponseHook>,
    mut continue_sent: bool,
    hops: HopByHop,
) -> Result<Relayed, HttpInterceptError>
where
    U: AsyncRead + Unpin,
    C: AsyncWrite + Unpin,
{
    let rules = scrub.and_then(|scrub| scrub.rules);
    let limit = scrub.map_or(BodyLimit::DEFAULT, |scrub| scrub.limit);
    let mut upstream = UpstreamReader {
        stream: upstream,
        buf: Vec::new(),
    };
    let mut client = ClientWriter {
        stream: client,
        written: 0,
    };
    let refused = hops
        .client()
        .unwrap_or_else(|| AnsweredRequest::new(method, true))
        .closing();

    loop {
        let head_len = read_response_header_end(upstream.stream, &mut upstream.buf).await?;
        let raw_head: Vec<u8> = upstream.buf.drain(..head_len).collect();
        let raw_headers = parse_response_headers(&latin1_text(&raw_head));

        let mut head_filter = Filter::new(rules, true);
        let mut head = head_filter.feed(&raw_head);
        head.extend(head_filter.finish());
        let mut scrubbed = head_filter.scrubbed();
        let unstripped = latin1_text(&head);
        let framing_headers = parse_response_headers(&unstripped);
        let head = match scrub {
            Some(scrub) => scrub.strip(unstripped.clone()),
            None => unstripped.clone(),
        };

        let status = ResponseStatus::parse(&head)?;
        check_header_lines(&head)?;
        let headers = parse_response_headers(&head);

        if status.is_interim() {
            if status.code == 100 && std::mem::take(&mut continue_sent) {
                continue;
            }
            client.send(&latin1_bytes(&head)).await?;
            continue;
        }

        let stripped = hops.is_stripped() && status.code != 101;
        if stripped && has_folded_line(&head) {
            return Err(malformed("folded response header"));
        }
        let wire_head = if stripped {
            strip_hop_by_hop(&head)
        } else {
            head.clone()
        };
        if let Some(hook) = hook {
            if stripped {
                hook.notify(status.code, &parse_response_headers(&strip_hop_by_hop(&unstripped)));
            } else {
                hook.notify(status.code, &framing_headers);
            }
        }
        let framing = response_framing(method, status, &framing_headers)?;
        let mut scrubbing = rules.is_some();
        if let Some(scope) = scrub.and_then(|scrub| scrub.scope)
            && scrubbing
            && !matches!(framing, Framing::Empty)
            && is_compressed(&framing_headers)
        {
            match refuse_or_flag(scope, &mut client, refused).await? {
                Some(relayed) => return Ok(relayed),
                None => scrubbing = false,
            }
        }
        let mut filter = Filter::new(rules, scrubbing);
        let mut connection = keep_alive_or_close(status, &headers);
        let mut ends_with_close = false;
        let answered = hops.client().filter(|_| stripped);
        let client_keeps_alive = std::sync::OnceLock::new();
        let client_head = |edit: Option<HeadEdit>| match answered {
            Some(request) => {
                let head = framed_response_head(&wire_head, status.code, edit);
                let _ = client_keeps_alive.set(head.keeps_alive(request));
                head.render(request, SystemTime::now())
            }
            None => edit.map_or_else(|| wire_head.clone(), |edit| edit_head(&wire_head, edit)),
        };
        let chunkable = match answered {
            Some(request) => request.version() == RequestVersion::Http11,
            None => status.http11,
        };
        let keeps_length = has_header(&headers, "content-length");

        match framing {
            Framing::Empty => client.send(&latin1_bytes(&client_head(None))).await?,
            Framing::Sized(length) if !scrubbing && keeps_length => {
                client.send(&latin1_bytes(&client_head(None))).await?;
                let mut remaining = length;
                while remaining > 0 {
                    let piece = upstream.piece(remaining).await?;
                    remaining -= piece.len();
                    client.send(&piece).await?;
                }
            }
            Framing::Sized(length) if keeps_length && limit.allows(length) => {
                let mut body = Vec::with_capacity(length);
                let mut remaining = length;
                while remaining > 0 {
                    let piece = upstream.piece(remaining).await?;
                    remaining -= piece.len();
                    body.extend(filter.feed(&piece));
                    if !limit.allows(body.len()) {
                        return refuse_overgrown(&mut client, refused).await;
                    }
                }
                body.extend(filter.finish());
                if !limit.allows(body.len()) {
                    return refuse_overgrown(&mut client, refused).await;
                }
                let edit = (body.len() != length).then_some(HeadEdit::SetLength(body.len()));
                client.send(&latin1_bytes(&client_head(edit))).await?;
                client.send(&body).await?;
            }
            Framing::Sized(length) if chunkable => {
                let head = client_head(Some(HeadEdit::DropLengthAndChunk));
                client.send(&latin1_bytes(&head)).await?;
                let mut remaining = length;
                while remaining > 0 {
                    let piece = upstream.piece(remaining).await?;
                    remaining -= piece.len();
                    client.chunk(&filter.feed(&piece)).await?;
                }
                client.chunk(&filter.finish()).await?;
                client.send(b"0\r\n\r\n").await?;
            }
            Framing::Sized(length) => {
                client
                    .send(&latin1_bytes(&client_head(Some(HeadEdit::DropLength))))
                    .await?;
                let mut remaining = length;
                while remaining > 0 {
                    let piece = upstream.piece(remaining).await?;
                    remaining -= piece.len();
                    client.send(&filter.feed(&piece)).await?;
                }
                client.send(&filter.finish()).await?;
                ends_with_close = true;
            }
            Framing::Chunked if stripped && !chunkable => {
                client
                    .send(&latin1_bytes(&client_head(Some(HeadEdit::DropLengthAndChunk))))
                    .await?;
                loop {
                    let line = upstream.line(MAX_CHUNK_LINE_BYTES).await?;
                    let size = parse_chunk_size(&line).map_err(|_| malformed("malformed chunk size in response"))?;
                    if size == 0 {
                        break;
                    }
                    let mut remaining = size;
                    while remaining > 0 {
                        let piece = upstream.piece(remaining).await?;
                        remaining -= piece.len();
                        client.send(&filter.feed(&piece)).await?;
                    }
                    if !upstream.line(MAX_CHUNK_LINE_BYTES).await?.is_empty() {
                        return Err(malformed("malformed chunk framing in response"));
                    }
                }
                client.send(&filter.finish()).await?;
                upstream.trailers().await?;
            }
            Framing::Chunked => {
                let edit = if stripped || !has_header(&headers, "transfer-encoding") {
                    Some(HeadEdit::DropLengthAndChunk)
                } else if keeps_length {
                    Some(HeadEdit::DropLength)
                } else {
                    None
                };
                client.send(&latin1_bytes(&client_head(edit))).await?;
                loop {
                    let line = upstream.line(MAX_CHUNK_LINE_BYTES).await?;
                    let size = parse_chunk_size(&line).map_err(|_| malformed("malformed chunk size in response"))?;
                    if size == 0 {
                        break;
                    }
                    let mut remaining = size;
                    while remaining > 0 {
                        let piece = upstream.piece(remaining).await?;
                        remaining -= piece.len();
                        client.chunk(&filter.feed(&piece)).await?;
                    }
                    if !upstream.line(MAX_CHUNK_LINE_BYTES).await?.is_empty() {
                        return Err(malformed("malformed chunk framing in response"));
                    }
                }
                client.chunk(&filter.finish()).await?;
                let mut trailers = filter.feed(&upstream.trailers().await?);
                trailers.extend(filter.finish());
                if stripped {
                    trailers.clear();
                }
                client.send(b"0\r\n").await?;
                client.send(&trailers).await?;
                client.send(b"\r\n").await?;
            }
            Framing::UntilClose if stripped && chunkable => {
                let head = client_head(Some(HeadEdit::DropLengthAndChunk));
                client.send(&latin1_bytes(&head)).await?;
                while let Some(piece) = upstream.until_close().await? {
                    client.chunk(&filter.feed(&piece)).await?;
                }
                client.chunk(&filter.finish()).await?;
                client.send(b"0\r\n\r\n").await?;
                ends_with_close = true;
            }
            Framing::UntilClose => {
                client.send(&latin1_bytes(&client_head(None))).await?;
                while let Some(piece) = upstream.until_close().await? {
                    client.send(&filter.feed(&piece)).await?;
                }
                client.send(&filter.finish()).await?;
                ends_with_close = true;
            }
        }

        scrubbed += filter.scrubbed();
        if let Some(scope) = scrub.and_then(|scrub| scrub.scope)
            && scrubbed > 0
        {
            tracing::warn!(
                host = scope.target().host(),
                port = scope.target().port(),
                scrubbed,
                "upstream response reflected secret values, replaced by their sentinels"
            );
        }
        let upstream_connection = if ends_with_close {
            connection = Connection::Close;
            Connection::Close
        } else {
            keep_alive_or_close(status, &raw_headers)
        };
        if let Some(keeps_alive) = client_keeps_alive.get() {
            connection = if *keeps_alive {
                Connection::KeepAlive
            } else {
                Connection::Close
            };
        }
        return Ok(Relayed {
            bytes: client.written,
            connection,
            upstream: upstream_connection,
            upstream_hint: header_values(&raw_headers, "keep-alive")
                .next()
                .map_or(KeepAliveHint::none(), KeepAliveHint::parse),
        });
    }
}

/// Handles a compressed response from a host that is scanned: `Some` when it
/// was refused and answered, `None` when the policy lets it through unscanned
/// after reporting it.
async fn refuse_or_flag<C>(
    scope: &SubstitutionScope,
    client: &mut ClientWriter<'_, C>,
    request: AnsweredRequest,
) -> Result<Option<Relayed>, HttpInterceptError>
where
    C: AsyncWrite + Unpin,
{
    if !scope.injects_here() {
        return Ok(None);
    }
    let policy = scope.compressed_response_policy();
    tracing::warn!(
        host = scope.target().host(),
        port = scope.target().port(),
        ?policy,
        "compressed response from a host with secret bindings cannot be scanned"
    );
    scope.report_compressed_response().await;
    match policy {
        CompressedResponsePolicy::Flag => Ok(None),
        CompressedResponsePolicy::Refuse => refuse(
            client,
            request,
            "response refused: compressed content from a host with secret bindings",
            "compressed-response",
        )
        .await
        .map(Some),
    }
}

async fn refuse_overgrown<C>(
    client: &mut ClientWriter<'_, C>,
    request: AnsweredRequest,
) -> Result<Relayed, HttpInterceptError>
where
    C: AsyncWrite + Unpin,
{
    refuse(
        client,
        request,
        "response refused: replacing secret values made the body exceed the size limit",
        "scrubbed-response-too-large",
    )
    .await
}

/// A response this proxy refuses to relay, answered with a 502 framed as
/// the workload's HTTP stack frames the 502 the proxy writes for an upstream it cannot relay
///, on a connection that ends after it.
async fn refuse<C>(
    client: &mut ClientWriter<'_, C>,
    request: AnsweredRequest,
    body: &str,
    tag: &str,
) -> Result<Relayed, HttpInterceptError>
where
    C: AsyncWrite + Unpin,
{
    let response = FramedAnswer::plain(
        StatusCode::BAD_GATEWAY.as_u16(),
        crate::deny_header_style::header_field(tag),
        body.to_string(),
    )
    .bytes(request, SystemTime::now());
    client.send(&response).await?;
    Ok(Relayed {
        bytes: client.written,
        connection: Connection::Close,
        upstream: Connection::Close,
        upstream_hint: KeepAliveHint::none(),
    })
}

pub(crate) struct WriteWatch<'a, C> {
    inner: &'a mut C,
    wrote: bool,
}

impl<'a, C> WriteWatch<'a, C> {
    pub(crate) fn new(inner: &'a mut C) -> Self {
        Self { inner, wrote: false }
    }

    pub(crate) fn wrote(&self) -> bool {
        self.wrote
    }
}

impl<C: AsyncWrite + Unpin> AsyncWrite for WriteWatch<'_, C> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut *self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = &polled
            && *written > 0
        {
            self.wrote = true;
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.inner).poll_shutdown(cx)
    }
}

#[derive(Clone, Copy)]
struct ResponseStatus {
    code: u16,
    http11: bool,
}

impl ResponseStatus {
    fn parse(head: &str) -> Result<Self, HttpInterceptError> {
        let line = head.split("\r\n").next().unwrap_or_default();
        let mut parts = line.split_whitespace();
        let version = parts.next().unwrap_or_default();
        let code = parts
            .next()
            .and_then(|code| code.parse::<u16>().ok())
            .filter(|code| (100..=999).contains(code))
            .ok_or_else(|| malformed("invalid response status line"))?;
        if !version.starts_with("HTTP/1.") {
            return Err(malformed("invalid response status line"));
        }
        Ok(Self {
            code,
            http11: version == "HTTP/1.1",
        })
    }

    fn is_interim(self) -> bool {
        (100..200).contains(&self.code) && self.code != 101
    }
}

fn check_header_lines(head: &str) -> Result<(), HttpInterceptError> {
    for line in head.split("\r\n").skip(1).take_while(|line| !line.is_empty()) {
        let value = if line.starts_with([' ', '\t']) {
            line
        } else {
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| malformed("invalid response header"))?;
            if name.is_empty() || !name.bytes().all(is_token_byte) {
                return Err(malformed("invalid response header name"));
            }
            value
        };
        if value.chars().any(|c| (c < ' ' && c != '\t') || c == '\x7f') {
            return Err(malformed("invalid response header value"));
        }
    }
    Ok(())
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn response_framing(
    method: &str,
    status: ResponseStatus,
    headers: &[(String, String)],
) -> Result<Framing, HttpInterceptError> {
    if method.eq_ignore_ascii_case("HEAD") || matches!(status.code, 204 | 304) {
        return Ok(Framing::Empty);
    }
    if status.code == 101 {
        return Ok(Framing::UntilClose);
    }
    let codings: Vec<&str> = header_values(headers, "transfer-encoding")
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|coding| !coding.is_empty())
        .collect();
    if let Some(last) = codings.last() {
        return Ok(if last.eq_ignore_ascii_case("chunked") {
            Framing::Chunked
        } else {
            Framing::UntilClose
        });
    }
    let mut length = None;
    for value in header_values(headers, "content-length").flat_map(|value| value.split(',')) {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed("invalid Content-Length"));
        }
        let value = value
            .parse::<usize>()
            .map_err(|_| malformed("invalid Content-Length"))?;
        if length.is_some_and(|seen| seen != value) {
            return Err(malformed("conflicting Content-Length"));
        }
        length = Some(value);
    }
    Ok(length.map_or(Framing::UntilClose, Framing::Sized))
}

fn is_compressed(headers: &[(String, String)]) -> bool {
    has_opaque_content_encoding(headers)
        || header_values(headers, "transfer-encoding")
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .any(|coding| {
                !coding.is_empty()
                    && !coding.eq_ignore_ascii_case("chunked")
                    && !coding.eq_ignore_ascii_case("identity")
            })
}

fn keep_alive_or_close(status: ResponseStatus, headers: &[(String, String)]) -> Connection {
    let has_token = |token: &str| {
        header_values(headers, "connection")
            .flat_map(|value| value.split(','))
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(token))
    };
    let keeps = if status.http11 {
        !has_token("close")
    } else {
        has_token("keep-alive")
    };
    if keeps {
        Connection::KeepAlive
    } else {
        Connection::Close
    }
}

fn header_values<'a>(headers: &'a [(String, String)], name: &'a str) -> impl Iterator<Item = &'a str> {
    headers
        .iter()
        .filter(move |(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    header_values(headers, name).next().is_some()
}

fn latin1_text(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

fn latin1_bytes(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u8).collect()
}

fn edit_head(head: &str, edit: HeadEdit) -> String {
    let block = head.strip_suffix("\r\n\r\n").unwrap_or(head);
    let mut lines: Vec<String> = Vec::new();
    let mut length_seen = false;
    for (index, line) in block.split("\r\n").enumerate() {
        let is_length = index > 0
            && line
                .split_once(':')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"));
        if !is_length {
            lines.push(line.to_string());
        } else if let HeadEdit::SetLength(length) = edit
            && !length_seen
        {
            lines.push(format!("Content-Length: {length}"));
        }
        length_seen |= is_length;
    }
    if matches!(edit, HeadEdit::DropLengthAndChunk) {
        lines.push("Transfer-Encoding: chunked".to_string());
    }
    let mut edited = lines.join("\r\n");
    edited.push_str("\r\n\r\n");
    edited
}

#[cfg(test)]
mod tests;
