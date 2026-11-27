//! TLS termination and re-encrypt splice for domain rules matched with a
//! `Transform` or `Forward` action: terminate TLS with the client using a
//! `SandboxCa`-issued leaf cert, open a fresh TLS connection to the real
//! upstream, and splice the two plaintext streams together.
//!
//! For a `Forward` rule, "the real upstream" is not one fixed connection: a
//! request matching the rule's `match_rules` is instead routed to a *second*,
//! independent TLS connection opened to `forward_url`'s own host, carrying an
//! OIDC identity token; a request on the same client connection that does
//! *not* match is routed to the connection's original (SNI-sniffed)
//! destination, unmodified, exactly as an unmatched `Transform` request is.
//! This is the forward-url behavior of hosted sandboxes, where only matched
//! traffic leaves via the forwarding hook and everything else continues to
//! its real destination. See [`terminate_and_forward`] for where both
//! connections are established.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::AsyncReadExt as _;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout_at;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use trogon_sandbox_contract::NetworkOptions;
use trogon_sandbox_contract::ResponseHeaderStrip;
use trogon_sandbox_contract::{CredentialTransform, LeaseId, RequestForward, SandboxId, TenantId};

use crate::credential_source::CredentialSource;
use crate::dial::{self, UpstreamMarking};
use crate::early_answer::UpstreamAfter;
use crate::http_intercept::{
    self, BodyLimit, CheckedRead, ClientBody, CompiledMatchRules, FirstHead, HttpInterceptError, Injection, NextHead,
    OriginalDestination, RequestGuard, TlsTarget, UpstreamRequest,
};
use crate::oidc_signer::{MintRequest, OidcSigner};
use crate::request_filter::RequestChecks;
use crate::resolve::VettedEndpoint;
use crate::response_relay::{Connection, HopByHop};
use crate::secret_substitution::SecretSubstitution;
use crate::sni::ClientServerName;
use crate::stream_use::StreamUse;
use crate::tls_ca::{SandboxCa, server_config_for_leaf};
pub use crate::tls_intercept_error::TlsInterceptError;
use crate::tunnel_deadlines::{
    DeadlineStream, GRACE_AFTER_FLUSH, SharedQueueBudget, StalledWrites, TunnelDeadlines, close_after_flush,
};
use crate::upstream_leg::{AfterDiscard, UpstreamLeg};

mod constants;
use constants::{MAX_CLIENT_HELLO_RECORD, TLS_HANDSHAKE_RECORD, TLS_RECORD_HEADER_LEN};

/// Wraps a client stream plus bytes already read off the wire while sniffing
/// SNI (see [`crate::proxy::sniff_client_hello`]), so a `TlsAcceptor` can
/// observe the full ClientHello from record byte 0 even though the proxy
/// already consumed those bytes from the socket during sniffing: reads
/// replay the buffered prefix first, then fall through to the live socket;
/// writes pass straight through since only the read side was pre-consumed.
pub struct PrefixedStream<S = TcpStream> {
    prefix: Vec<u8>,
    prefix_pos: usize,
    inner: S,
}

impl<S> PrefixedStream<S> {
    pub fn new(inner: S, prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            prefix_pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.prefix_pos < self.prefix.len() {
            let remaining = &self.prefix[self.prefix_pos..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.prefix_pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Carries a matched `Transform` domain rule's credential-injection details
/// (see [`trogon_sandbox_contract::CredentialTransform`]) plus the
/// [`CredentialSource`] used to fetch the broker-issued token, so
/// [`terminate_and_forward`] can hand both to
/// [`crate::http_intercept::run_http_intercept`].
pub struct CredentialInjectionContext<'a> {
    pub transform: &'a CredentialTransform,
    pub credential_source: &'a dyn CredentialSource,
}

/// Carries a matched `Forward` domain rule (see
/// [`trogon_sandbox_contract::RequestForward`]) plus this running process's own
/// OIDC-minting identity (see [`crate::proxy::OidcIdentityContext`]) and the
/// connection's original destination scheme/port (its host is `sni_host`,
/// already a parameter of [`terminate_and_forward`]), so
/// [`terminate_and_forward`] can route matched requests to `forward.forward_url`
/// with a freshly minted identity token, and unmatched requests to the
/// original destination, per request.
pub struct ForwardContext<'a> {
    pub forward: &'a RequestForward,
    pub signer: &'a OidcSigner,
    pub issuer: &'a str,
    pub subject: &'a SandboxId,
    pub tenant_id: &'a TenantId,
    pub lease_id: &'a LeaseId,
    pub original_scheme: &'a str,
    pub original_port: u16,
    pub marking: UpstreamMarking,
}

/// Terminates TLS with the client (using a fresh `ca`-issued leaf cert for
/// `sni_host`), opens a genuine re-encrypted TLS connection to
/// `upstream` (validated by `upstream_tls_config`, not by `ca`), and
/// forwards traffic between the two decrypted plaintext streams.
///
/// When `credential_injection` is `Some`, each HTTP/1.1 request on the
/// connection is parsed and matched against the transform's `match_rules` so
/// a broker-issued credential can be injected before the request reaches
/// upstream (see [`crate::http_intercept::run_http_intercept`]).
///
/// When `forward` is `Some`, each HTTP/1.1 request on the connection is
/// parsed and matched against the rule's `match_rules`: a matched request is
/// routed to a *second*, independent TLS connection to `forward.forward_url`'s
/// own host (opened lazily, only once a request actually matches, since most
/// connections carrying a `Forward` rule may never see a matching request)
/// with `trogon-forwarded-*`/`trogon-sandbox-oidc-token` headers injected (see
/// `crate::http_intercept::build_forwarded_request`); an unmatched request
/// is forwarded to `upstream` (the connection's real, original
/// destination) unmodified, the same as an unmatched `Transform` request,
/// since only traffic the rule author explicitly opted into forwarding should
/// ever leave via the forwarding hook.
///
/// `credential_injection` and `forward` are mutually exclusive in practice (a
/// domain rule's action is one of `Transform` or `Forward`, never both; see
/// [`trogon_sandbox_contract::DomainRuleAction`]) but are accepted as two
/// independent `Option`s here rather than a shared enum, since the enum
/// already exists one layer up as `DomainAction` in [`crate::proxy`] and
/// duplicating it here would only add an unreachable case to match on.
///
/// Returns the byte counts in each direction on success.
#[allow(clippy::too_many_arguments)]
pub async fn terminate_and_forward<S>(
    client_stream: PrefixedStream<S>,
    sni_host: &str,
    ca: &SandboxCa,
    upstream: VettedEndpoint,
    marking: UpstreamMarking,
    upstream_tls_config: Arc<rustls::ClientConfig>,
    credential_injection: Option<CredentialInjectionContext<'_>>,
    forward: Option<ForwardContext<'_>>,
) -> Result<(u64, u64), TlsInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    terminate_and_forward_with_secrets(
        client_stream,
        sni_host,
        ca,
        upstream,
        marking,
        upstream_tls_config,
        credential_injection,
        forward,
        None,
        TunnelDeadlines::default(),
    )
    .await
}

/// [`terminate_and_forward`] that also substitutes the lease's secret
/// sentinels in each request, gated on the TLS target (`sni_host` and the
/// port of `upstream`). With no `Transform` or `Forward` rule the
/// connection is still relayed request by request so substitution applies.
#[allow(clippy::too_many_arguments)]
pub async fn terminate_and_forward_with_secrets<S>(
    client_stream: PrefixedStream<S>,
    sni_host: &str,
    ca: &SandboxCa,
    upstream: VettedEndpoint,
    marking: UpstreamMarking,
    upstream_tls_config: Arc<rustls::ClientConfig>,
    credential_injection: Option<CredentialInjectionContext<'_>>,
    forward: Option<ForwardContext<'_>>,
    secrets: Option<&SecretSubstitution>,
    deadlines: TunnelDeadlines,
) -> Result<(u64, u64), TlsInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    terminate_and_forward_checked(
        client_stream,
        sni_host,
        ca,
        upstream.into(),
        marking,
        upstream_tls_config,
        credential_injection,
        forward,
        secrets,
        deadlines,
        None,
        None,
        None,
        None,
    )
    .await
}

/// Reads the first TLS record off `stream` without consuming it for good: the
/// bytes come back so the caller can replay them into the handshake. Anything
/// that is not a bounded handshake record comes back as just its header.
async fn read_client_hello<S>(stream: &mut S) -> io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = vec![0u8; TLS_RECORD_HEADER_LEN];
    stream.read_exact(&mut bytes).await?;
    let record_len = usize::from(u16::from_be_bytes([bytes[3], bytes[4]]));
    if bytes[0] != TLS_HANDSHAKE_RECORD || record_len > MAX_CLIENT_HELLO_RECORD {
        return Ok(bytes);
    }
    bytes.resize(TLS_RECORD_HEADER_LEN + record_len, 0);
    stream.read_exact(&mut bytes[TLS_RECORD_HEADER_LEN..]).await?;
    Ok(bytes)
}

/// [`terminate_and_forward_with_secrets`] that also runs `checks` on each
/// decrypted request, answering a denied one with 403 before anything
/// reaches the upstream.
#[allow(clippy::too_many_arguments)]
pub async fn terminate_and_forward_checked<S>(
    client_stream: PrefixedStream<S>,
    sni_host: &str,
    ca: &SandboxCa,
    upstream: dial::LegAddresses,
    marking: UpstreamMarking,
    upstream_tls_config: Arc<rustls::ClientConfig>,
    credential_injection: Option<CredentialInjectionContext<'_>>,
    forward: Option<ForwardContext<'_>>,
    secrets: Option<&SecretSubstitution>,
    deadlines: TunnelDeadlines,
    checks: Option<RequestChecks>,
    network_strip: Option<Arc<ResponseHeaderStrip>>,
    shared_queue: Option<SharedQueueBudget>,
    network_options: Option<&NetworkOptions>,
) -> Result<(u64, u64), TlsInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let match_rules = match (&credential_injection, &forward) {
        (Some(context), _) => CompiledMatchRules::compile(&context.transform.match_rules),
        (None, Some(context)) => CompiledMatchRules::compile(&context.forward.match_rules),
        (None, None) => CompiledMatchRules::compile(&[]),
    }
    .map_err(HttpInterceptError::from)?;
    let target = TlsTarget::new(sni_host, upstream.port());
    let guard = RequestGuard::new(target.clone())
        .with_substitution(secrets.and_then(|secrets| secrets.scope_for(&target)))
        .with_request_checks(checks)
        .with_network_strip(network_strip)
        .with_keep_alive(deadlines.keep_alive);
    let mut guard = match network_options {
        Some(options) => {
            let guard = guard.with_ambiguous_targets(options.ambiguous_targets);
            match options.request_body_cap {
                Some(cap) => guard.with_body_limit(BodyLimit::new(cap.bytes())),
                None => guard,
            }
        }
        None => guard,
    };

    let mut client_stream = client_stream;
    let handshake_deadline = deadlines.handshake_deadline();
    let hello = timeout_at(handshake_deadline, read_client_hello(&mut client_stream))
        .await
        .map_err(|_| TlsInterceptError::ClientHandshakeTimedOut {
            host: sni_host.to_owned(),
            port: upstream.port(),
            wait: deadlines.handshake,
        })?
        .map_err(TlsInterceptError::ClientHandshake)?;
    let hello_at = tokio::time::Instant::now();
    let server_name = ClientServerName::of_client_hello(&hello);
    let leaf_name = sni_host.to_owned();
    guard = guard.with_client_server_name(server_name);
    let client_stream = PrefixedStream::new(client_stream, hello);

    let server_name = rustls::pki_types::ServerName::try_from(sni_host.to_string()).map_err(|source| {
        TlsInterceptError::InvalidServerName {
            hostname: sni_host.to_string(),
            source,
        }
    })?;

    let certified_key = ca
        .mint_leaf_cert(&leaf_name)
        .map_err(TlsInterceptError::LeafCertificate)?;
    let server_config = server_config_for_leaf(certified_key);
    let acceptor = TlsAcceptor::from(server_config);

    let mut client_tls = timeout_at(handshake_deadline, acceptor.accept(client_stream))
        .await
        .map_err(|_| TlsInterceptError::ClientHandshakeTimedOut {
            host: sni_host.to_owned(),
            port: upstream.port(),
            wait: deadlines.handshake,
        })?
        .map_err(TlsInterceptError::ClientHandshake)?;
    guard = guard
        .with_client_sni(client_tls.get_ref().1.server_name().map(str::to_owned))
        .with_first_head(FirstHead::counted_from(hello_at, deadlines.first_request));
    client_tls
        .get_mut()
        .1
        .set_buffer_limit(Some(deadlines.max_queued_for_client.bytes()));
    let mut client_tls = DeadlineStream::new(client_tls, deadlines);
    if let Some(shared) = shared_queue {
        client_tls = client_tls.with_shared_budget(shared);
    }

    let connector = TlsConnector::from(Arc::clone(&upstream_tls_config));
    let stalled = deadlines.stalled;
    let leg = Arc::new(dial::LegDialer::new(upstream, marking));
    let upstream_leg = UpstreamLeg::on_demand(Box::new(move || {
        let connector = connector.clone();
        let server_name = server_name.clone();
        let leg = Arc::clone(&leg);
        Box::pin(async move {
            let dialed = async {
                let tcp = leg.connect().await.map_err(HttpInterceptError::UpstreamConnect)?;
                connector
                    .connect(server_name, tcp)
                    .await
                    .map(|tls| StalledWrites::new(tls, stalled))
                    .map_err(HttpInterceptError::UpstreamHandshake)
            }
            .await;
            if dialed.is_err() {
                leg.forget();
            }
            dialed
        })
    }));

    let relayed = async {
        match (credential_injection, forward) {
            (
                Some(CredentialInjectionContext {
                    transform,
                    credential_source,
                }),
                _,
            ) => {
                let counts = http_intercept::run_compiled_http_intercept(
                    &mut client_tls,
                    upstream_leg,
                    Some(Injection {
                        transform,
                        match_rules: &match_rules,
                        credential_source,
                    }),
                    &guard,
                )
                .await?;
                Ok((counts.client_to_upstream, counts.upstream_to_client))
            }
            (None, Some(forward_context)) => {
                run_forward_intercept(
                    &mut client_tls,
                    upstream_leg,
                    sni_host,
                    upstream_tls_config,
                    forward_context,
                    &match_rules,
                    &guard,
                )
                .await
            }
            (None, None) => {
                let counts =
                    http_intercept::run_compiled_http_intercept(&mut client_tls, upstream_leg, None, &guard).await?;
                Ok((counts.client_to_upstream, counts.upstream_to_client))
            }
        }
    }
    .await;
    if relayed.is_ok() && !guard.ended_destroyed() {
        let end = close_after_flush(
            &mut client_tls,
            deadlines.stalled,
            guard.grace_after_idle_end().unwrap_or(GRACE_AFTER_FLUSH),
        )
        .await;
        tracing::debug!(host = sni_host, %end, "terminated tunnel closed");
    }
    relayed
}

/// Drives one TLS-terminated connection carrying a `Forward` domain rule:
/// reads HTTP/1.1 requests off `client_tls`, and for each one either routes
/// it to a lazily-opened connection to `forward.forward.forward_url`'s host
/// (matched) or relays it to `original_upstream_tls` (unmatched), looping
/// until either side closes. See [`terminate_and_forward`]'s doc comment for
/// why unmatched requests fall back to the original destination rather than
/// erroring.
async fn run_forward_intercept<C, U>(
    mut client_tls: C,
    mut original_upstream: UpstreamLeg<U>,
    sni_host: &str,
    upstream_tls_config: Arc<rustls::ClientConfig>,
    forward: ForwardContext<'_>,
    match_rules: &CompiledMatchRules,
    guard: &RequestGuard,
) -> Result<(u64, u64), TlsInterceptError>
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let mut counts_client_to_upstream: u64 = 0;
    let mut counts_upstream_to_client: u64 = 0;
    let mut client_buf: Vec<u8> = Vec::new();
    let mut forward_target: Option<tokio_rustls::client::TlsStream<TcpStream>> = None;

    let mut idle_since = None;
    loop {
        match guard.next_head(&mut client_tls, &mut client_buf, idle_since).await {
            NextHead::Arrived => {}
            NextHead::Ended => break,
            NextHead::Idle => match idle_since {
                Some(_) => break,
                None => return Err(guard.no_first_request().into()),
            },
        }
        let mode = if guard.streams_bodies() {
            http_intercept::BodyMode::Stream
        } else {
            http_intercept::BodyMode::Hold
        };
        let mut request =
            match http_intercept::read_checked_request_as(&mut client_tls, &mut client_buf, guard, mode).await? {
                CheckedRead::Request(request) => *request,
                CheckedRead::Answered => {
                    idle_since = Some(tokio::time::Instant::now());
                    continue;
                }
                CheckedRead::Closed => break,
            };

        if match_rules.matches(&request) {
            match request.hold_body(&mut client_tls, &mut client_buf, guard).await {
                Ok(()) => {}
                Err(HttpInterceptError::Refused(refusal)) => {
                    http_intercept::answer_refusal(&mut client_tls, request.answered(), refusal).await?;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
            let target = match &mut forward_target {
                Some(connected) => connected,
                empty => empty.insert(
                    connect_forward_target(
                        forward.forward.forward_url.as_str(),
                        &upstream_tls_config,
                        forward.marking,
                    )
                    .await?,
                ),
            };

            let token = forward
                .signer
                .mint(MintRequest {
                    issuer: forward.issuer,
                    subject: forward.subject,
                    tenant_id: forward.tenant_id,
                    lease_id: forward.lease_id,
                    audience: forward.forward.forward_url.as_str(),
                    ttl_seconds: forward.forward.ttl_seconds,
                })
                .map_err(TlsInterceptError::OidcMint)?;

            let original_destination = OriginalDestination {
                host: sni_host,
                scheme: forward.original_scheme,
                port: forward.original_port,
            };
            let outgoing = http_intercept::build_forwarded_request(&request, &original_destination, &token);

            target
                .write_all(&outgoing)
                .await
                .map_err(TlsInterceptError::UpstreamConnect)?;
            // The response is awaited next, so a TLS record left buffered
            // here would never reach the upstream.
            target.flush().await.map_err(TlsInterceptError::UpstreamConnect)?;
            counts_client_to_upstream += outgoing.len() as u64;

            let relayed = http_intercept::relay_response_or_bad_gateway(
                target,
                &mut client_tls,
                request.method(),
                None,
                request.response_hook(),
                request.expects_continue(),
                HopByHop::Relayed,
                StreamUse::Fresh,
            )
            .await?;
            counts_upstream_to_client += relayed.bytes;
            idle_since = Some(tokio::time::Instant::now());
            if relayed.connection == Connection::Close || request.closes_connection() {
                break;
            }
        } else {
            let prepared = match http_intercept::prepare_upstream_request(&mut client_tls, &request, guard).await? {
                UpstreamRequest::Refused => break,
                prepared => prepared,
            };
            let to_upstream = prepared.resolve(&request);
            let (original_upstream_tls, stream_use) = match original_upstream.stream().await {
                Ok(taken) => taken,
                Err(error) => {
                    let closes = request.closes_connection() || !original_upstream.can_redial();
                    http_intercept::answer_lost_upstream(&mut client_tls, &mut client_buf, to_upstream, closes, error)
                        .await?;
                    idle_since = Some(tokio::time::Instant::now());
                    continue;
                }
            };
            // The response is awaited next, so a TLS record left buffered here
            // would never reach the upstream.
            let sent_head = async {
                original_upstream_tls.write_all(to_upstream.raw_bytes()).await?;
                original_upstream_tls.flush().await
            };
            if let Err(error) = sent_head.await {
                let error = HttpInterceptError::UpstreamWrite(error);
                if error.ends_silently(stream_use) {
                    return Err(error.into());
                }
                let closes = request.closes_connection() || original_upstream.discard() == AfterDiscard::Ends;
                http_intercept::answer_lost_upstream(&mut client_tls, &mut client_buf, to_upstream, closes, error)
                    .await?;
                idle_since = Some(tokio::time::Instant::now());
                continue;
            }
            counts_client_to_upstream += to_upstream.raw_bytes().len() as u64;
            let exchanged = match http_intercept::exchange(
                &mut client_tls,
                &mut client_buf,
                original_upstream_tls,
                to_upstream,
                request.substitution(),
                request.method(),
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
                    http_intercept::answer_refusal(&mut client_tls, request.answered(), refusal).await?;
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            counts_client_to_upstream += exchanged.body_bytes;
            counts_upstream_to_client += exchanged.relayed.bytes;
            idle_since = Some(exchanged.answered_at);
            let leg_ends = match exchanged.upstream {
                UpstreamAfter::Spent => original_upstream.discard() == AfterDiscard::Ends,
                UpstreamAfter::Reusable => {
                    original_upstream.served();
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
    }

    Ok((counts_client_to_upstream, counts_upstream_to_client))
}

/// Parses `forward_url`, opens a fresh TCP connection to its host (default
/// port 443 if unspecified, since `forward_url` is always an `https://` URL
/// carrying a bearer-equivalent OIDC token that must never be sent in the
/// clear), and performs a TLS handshake validated by `upstream_tls_config`
/// (the process's normal system root store, exactly as any other upstream
/// connection this proxy makes; `forward_url`'s host is not, and must not be,
/// trusted via `SandboxCa`, which only ever signs leaves shown to the
/// sandboxed client).
async fn connect_forward_target(
    forward_url: &str,
    upstream_tls_config: &Arc<rustls::ClientConfig>,
    marking: UpstreamMarking,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, TlsInterceptError> {
    let parsed = url::Url::parse(forward_url).map_err(|err| TlsInterceptError::InvalidForwardUrl {
        forward_url: forward_url.to_string(),
        reason: err.to_string(),
    })?;

    let host = parsed
        .host_str()
        .ok_or_else(|| TlsInterceptError::InvalidForwardUrl {
            forward_url: forward_url.to_string(),
            reason: "missing host".to_string(),
        })?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);

    let server_name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|source| {
        TlsInterceptError::InvalidServerName {
            hostname: host.clone(),
            source,
        }
    })?;

    let tcp = dial::connect_declared(marking, host.as_str(), port)
        .await
        .map_err(|source| TlsInterceptError::ForwardConnect {
            host: host.clone(),
            source,
        })?;

    let connector = TlsConnector::from(Arc::clone(upstream_tls_config));
    connector
        .connect(server_name, tcp)
        .await
        .map_err(|source| TlsInterceptError::ForwardHandshake { host, source })
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod forward_tests;
