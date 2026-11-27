//! Plain HTTP for the explicit proxy: absolute-form requests relayed to the
//! origin one at a time. Each request on a connection is parsed with the same
//! checks a terminated request gets, authenticated, admitted by the policy,
//! run through the request rules, and only then sent upstream, with its body
//! streamed in its own framing.

use crate::host_kind::{HostClassification, HostKind};
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::{
    DecideMethod, DomainRuleAction, EgressViolationReason, HttpMethod, NetworkPolicy, RefusedRequest,
};
use trogon_sandbox_protocol::response_strip;

use super::admission::Admission;
use super::connect;
use super::denied::denied;
use super::denied_request::denied_request;
use super::ingress_core::Ingress;
use super::unreachable::unreachable;
use super::{ClientIo, ConnectTarget, IngressOutcome, IngressRejectionError};
use crate::body_limit::BodyLimit;
use crate::framed_head::{AnsweredRequest, FramedAnswer, parsed_fields};
use crate::framing::Framing;
use crate::head_parse_error::HeadParseError;
use crate::header_mutator::mutated;
use crate::http_intercept::{
    ProxiedHead, TargetMode, answer_denial, answer_refusal, bad_gateway_answer, buffered_message_len,
    check_chunked_start, drain_body_whole, expects_continue, framing_after_continue, framing_of_refused_head,
    pipelined_error, read_proxied_head, read_ready, refused_target_parts, relay_rechunked_body_without_trailers,
    relay_request_body, relay_response_hops, relay_scanned_body_without_trailers, substitute_proxied,
};
use crate::http_intercept_error::HttpInterceptError;
use crate::proxy::{ProxyOutcome, TlsInterceptContext};
use crate::refusal_error::RefusalError;
use crate::request_body::{RequestBody, StreamedBody};
use crate::request_decision::{RequestDenial, fold_header_name};
use crate::request_filter::{RequestChecks, RequestDecision, RequestScheme, RequestView};
use crate::response_relay::{Connection, HopByHop, ResponseScrub, WriteWatch};
use crate::tls_target::TlsTarget;

mod constants;
use constants::{DRAIN_TIMEOUT, HOP_BY_HOP, HTTP_DEFAULT_PORT, MAX_DRAIN_BYTES};

/// The request's fields as the workload's HTTP stack's request headers hold them, which is what
/// the proxy forwards: names lowercased, a repeated
/// field folded into its first place ([`parsed_fields`]), and `Host` rebuilt from the target where the client
/// put it, or after the others when it sent none.
fn framed_headers(headers: &[(String, String)], authority: &str) -> Vec<(String, String)> {
    let mut view = parsed_fields(headers);
    match view.iter_mut().find(|(name, _)| name == "host") {
        Some((_, value)) => *value = authority.to_string(),
        None => view.push(("host".to_string(), authority.to_string())),
    }
    view
}

enum Step {
    Next {
        target: ConnectTarget,
        dest_ip: Option<IpAddr>,
        sent: u64,
        received: u64,
        keep_alive: bool,
    },
    /// A request answered by the proxy through the workload's HTTP stack, which keeps the
    /// connection for the next request when the workload's HTTP stack would.
    Answered {
        outcome: IngressOutcome,
        keep_alive: bool,
    },
    End(IngressOutcome),
}

/// What is left of a request's body when the proxy answers it itself.
enum Unread<'a> {
    Nothing,
    BodyOf { head: &'a [u8], continued: bool },
}

impl Ingress {
    /// Serves the plain requests of one authenticated connection until the
    /// client or the origin ends it, the client asks to close, or a request is
    /// answered in a way the workload's HTTP stack closes after. The first outcome that is
    /// not a completed request is the connection's outcome.
    pub(super) async fn serve_plain<S: ClientIo>(
        &self,
        mut client: S,
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
        first_head: Vec<u8>,
        mut buffered: Vec<u8>,
    ) -> IngressOutcome {
        let checks = tls_intercept.request_checks(policy);
        let mut head = first_head;
        let mut first = true;
        let mut uploaded = 0u64;
        let mut downloaded = 0u64;
        let mut last = None;
        let mut answered = None;

        loop {
            if !first {
                if buffered.is_empty() {
                    let mut byte = [0u8; 1];
                    match timeout(self.idle_wait(), client.read(&mut byte)).await {
                        Ok(Ok(1)) => buffered.push(byte[0]),
                        _ => break,
                    }
                }
                let next = timeout(
                    self.limits.head_timeout,
                    connect::read_next_head(&mut client, &mut buffered, self.limits.max_head_bytes),
                )
                .await;
                match next {
                    Ok(Ok(Some(next))) => head = next,
                    Ok(Ok(None)) | Err(_) => break,
                    Ok(Err(rejection)) => return IngressOutcome::Rejected(rejection),
                }
                if let Err(rejection) = connect::reauthenticate(
                    &mut client,
                    &head,
                    self.token.as_ref(),
                    tls_intercept.network_options.reasoned_refusals.is_enabled(),
                )
                .await
                {
                    return IngressOutcome::Rejected(rejection);
                }
            }
            first = false;

            match self
                .serve_one(&mut client, &mut buffered, &head, policy, tls_intercept, &checks)
                .await
            {
                Step::End(outcome) => return outcome,
                Step::Answered { outcome, keep_alive } => {
                    answered.get_or_insert(outcome);
                    if !keep_alive {
                        drain_unread(&mut client).await;
                        break;
                    }
                }
                Step::Next {
                    target,
                    dest_ip,
                    sent,
                    received,
                    keep_alive,
                } => {
                    uploaded += sent;
                    downloaded += received;
                    last = Some((target, dest_ip));
                    if !keep_alive {
                        break;
                    }
                }
            }
        }

        let _ = client.shutdown().await;
        if let Some(outcome) = answered {
            return outcome;
        }
        // The first request is always served, so a connection that gets here served one.
        #[rustfmt::skip]
        let Some((target, dest_ip)) = last else { return IngressOutcome::Rejected(IngressRejectionError::Malformed("no request")) };
        IngressOutcome::Proxied {
            target,
            dest_ip,
            outcome: ProxyOutcome::Completed {
                bytes_client_to_upstream: uploaded,
                bytes_upstream_to_client: downloaded,
            },
        }
    }

    async fn serve_one<S: ClientIo>(
        &self,
        client: &mut S,
        buffered: &mut Vec<u8>,
        head: &[u8],
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
        checks: &Option<RequestChecks>,
    ) -> Step {
        let answered = AnsweredRequest::of_head(head);
        let request_target_mode = if checks.is_some() {
            TargetMode::Judged(tls_intercept.network_options.ambiguous_targets)
        } else {
            TargetMode::ParsedUrl
        };
        let mut parsed = match read_proxied_head(head, request_target_mode) {
            Ok(parsed) => parsed,
            Err(refusal) => {
                if let Some(checks) = checks
                    && matches!(
                        refusal,
                        RefusalError::MalformedTarget | RefusalError::TargetBytes | RefusalError::AmbiguousTarget
                    )
                    && let Some((method, authority, spelled)) = refused_target_parts(head)
                    && let Ok(target) = connect::parse_authority(&authority, Some(HTTP_DEFAULT_PORT))
                {
                    let refused = RefusedRequest::new(
                        &method,
                        &format!("http://{}{spelled}", authority_for(&target)),
                        &refusal.to_string(),
                    );
                    checks
                        .report_denied(&target.host().to_string(), target.port().get(), refused)
                        .await;
                }
                return Step::End(refuse(client, answered, refusal).await);
            }
        };
        if parsed.framing != Framing::None {
            read_ready(client, buffered).await;
        }
        if parsed.framing == Framing::Chunked
            && !expects_continue(&parsed.headers)
            && let Err(error) = check_chunked_start(client, buffered, 0).await
        {
            return Step::End(match error {
                HttpInterceptError::Refused(refusal) => refuse(client, answered, refusal).await,
                _ => IngressOutcome::Rejected(IngressRejectionError::Malformed("connection closed mid-request")),
            });
        }
        if let Some(error) = buffered_message_len(parsed.framing, buffered)
            .and_then(|end| pipelined_error(answered.keeps_alive(), &buffered[end..]))
        {
            return Step::End(refuse(client, answered, RefusalError::UnparsedBody(error)).await);
        }
        if self.limits.ascii_hosts_only
            && (parsed.authority.contains('@') || !connect::is_ascii_authority(&parsed.authority))
        {
            return Step::End(
                refuse(
                    client,
                    answered,
                    RefusalError::MalformedRequest("destination host is not plain ASCII"),
                )
                .await,
            );
        }
        let authority = match parsed.authority.rsplit_once('@') {
            Some((_, host)) if tls_intercept.network_options.reasoned_refusals.is_enabled() => host,
            _ => parsed.authority.as_str(),
        };
        let target = match connect::parse_authority(authority, Some(HTTP_DEFAULT_PORT)) {
            Ok(target) => target,
            Err(_) => return Step::End(refuse(client, answered, RefusalError::MalformedTarget).await),
        };
        // The head parser only lets a method token through, so this does not refuse in practice.
        #[rustfmt::skip]
        let Ok(method) = HttpMethod::new(&parsed.method) else { return Step::End(refuse(client, answered, RefusalError::Unparsed(HeadParseError::InvalidMethod)).await) };

        let (dest, fallbacks) = match self.admit(policy, &target, DecideMethod::Http(method.clone())).await {
            Admission::Denied {
                dest_ip,
                reason,
                explanation,
            } => {
                tls_intercept.explain_denial(target.host(), target.port().get(), explanation.as_ref());
                let answer = match connect::address_denial_words(&reason, explanation.as_ref()) {
                    Some(words) => connect::address_denied_plain_answer(target.host(), &words),
                    None => connect::blocked_plain_answer(),
                };
                let keep_alive = answer_in_place(
                    client,
                    buffered,
                    &answer,
                    answered,
                    Unread::BodyOf { head, continued: false },
                )
                .await;
                return Step::Answered {
                    outcome: denied(target, dest_ip, reason),
                    keep_alive,
                };
            }
            Admission::Unresolved(err) => {
                let keep_alive = answer_in_place(
                    client,
                    buffered,
                    &bad_gateway_answer(),
                    answered,
                    Unread::BodyOf { head, continued: false },
                )
                .await;
                return Step::Answered {
                    outcome: unreachable(target, err),
                    keep_alive,
                };
            }
            Admission::Allowed {
                dest,
                action: Some(DomainRuleAction::Transform(_) | DomainRuleAction::Forward(_)),
                ..
            } => {
                let keep_alive = answer_in_place(
                    client,
                    buffered,
                    &connect::blocked_plain_answer(),
                    answered,
                    Unread::BodyOf { head, continued: false },
                )
                .await;
                return Step::Answered {
                    outcome: denied(target, dest.reported_ip(), EgressViolationReason::PolicyDenied),
                    keep_alive,
                };
            }
            Admission::Allowed { dest, fallbacks, .. } => (dest, fallbacks),
        };

        {
            let canonical = match target.host().kind() {
                HostKind::Ip(ip) => ip.to_string(),
                HostKind::Domain(domain) => domain.as_str().to_string(),
            };
            if let Err(mismatch) = TlsTarget::new(&canonical, target.port().get())
                .check_plain_hosts(&parsed.host_headers, HTTP_DEFAULT_PORT)
            {
                let refusal = RefusalError::HostMismatch(mismatch);
                let refused = refused_request(
                    &target,
                    &method,
                    &parsed.target,
                    &RequestDenial::new().reason(refusal.to_string()),
                );
                let keep_alive = answer_in_place(
                    client,
                    buffered,
                    &refusal.framed_answer(),
                    answered,
                    Unread::BodyOf { head, continued: false },
                )
                .await;
                return Step::Answered {
                    outcome: denied_request(target, dest.reported_ip(), refused),
                    keep_alive,
                };
            }
        }

        let mut response_hook = None;
        let continued = AtomicBool::new(false);
        let expects_continue = parsed
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("expect") && value.eq_ignore_ascii_case("100-continue"));
        if let Some(checks) = checks {
            let (path, query) = match parsed.target.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (parsed.target.as_str(), None),
            };
            let verdict = {
                let source = StreamedBody::new(
                    &mut *client,
                    &mut *buffered,
                    parsed.framing,
                    expects_continue,
                    &continued,
                );
                checks
                    .decide(&RequestView {
                        host: target.host(),
                        port: target.port().get(),
                        method: &method,
                        path,
                        query,
                        sni: None,
                        headers: &parsed.headers,
                        body: Some(RequestBody::streamed(&source)),
                        scheme: RequestScheme::Http,
                        forms: Some(&parsed.forms),
                    })
                    .await
            };
            parsed.headers = framed_headers(&parsed.headers, &authority_for(&target));
            match verdict {
                RequestDecision::Allow => {}
                RequestDecision::AllowWith(allow) => {
                    if allow.header_edits().sets_headers() && !checks.plaintext_header_set() {
                        let denial = RequestDenial::plaintext_header_set();
                        let _ = answer_denial(client, answered, &denial).await;
                        drain_unread(client).await;
                        let refused = refused_request(&target, &method, &parsed.target, &denial);
                        return Step::End(denied_request(target, dest.reported_ip(), refused));
                    }
                    allow.header_edits().apply(&mut parsed.headers);
                    response_hook = allow.response_hook().cloned();
                }
                RequestDecision::DenyWith(denial) => {
                    let _ = answer_denial(client, answered, &denial).await;
                    drain_unread(client).await;
                    let refused = refused_request(&target, &method, &parsed.target, &denial);
                    return Step::End(denied_request(target, dest.reported_ip(), refused));
                }
                RequestDecision::Deny => {
                    let _ = answer_refusal(client, answered, RefusalError::RequestDenied).await;
                    drain_unread(client).await;
                    let refused = refused_request(&target, &method, &parsed.target, &RequestDenial::new());
                    return Step::End(denied_request(target, dest.reported_ip(), refused));
                }
            }
            if let Some(mutator) = checks.plaintext_mutator() {
                let edited = Host::new(&target.host().to_string())
                    .ok()
                    .and_then(|destination| mutated(mutator, &parsed.headers, &destination).ok());
                match edited {
                    Some(headers) => parsed.headers = headers,
                    None => {
                        let _ = answer_refusal(client, answered, RefusalError::RequestDenied).await;
                        drain_unread(client).await;
                        return Step::End(denied(target, dest.reported_ip(), EgressViolationReason::RequestDenied));
                    }
                }
            }
        }

        if checks.is_none() {
            parsed.headers = framed_headers(&parsed.headers, &authority_for(&target));
        }

        // Bindings scoped to a path or method are decided per request, as on a
        // terminated tunnel; the connection-level scope alone never injects them.
        let request_path = parsed
            .target
            .split_once('?')
            .map_or(parsed.target.as_str(), |(path, _)| path);
        let scope = tls_intercept
            .plaintext_scope(&target.host().to_string(), target.port().get())
            .map(|scope| scope.for_request(&parsed.method, request_path));
        let mut body_scan = None;
        if let Some(scope) = &scope {
            if expects_continue
                && parsed.framing != Framing::None
                && !continued.swap(true, Ordering::SeqCst)
                && client.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await.is_err()
            {
                return Step::End(client_gone(target, dest.reported_ip(), 0));
            }
            match substitute_proxied(&mut parsed, scope).await {
                Ok(scan) => body_scan = scan,
                Err(error) => {
                    return Step::End(refused_or_gone(client, answered, error, target, dest.reported_ip(), 0).await);
                }
            }
        }

        let dialed = timeout(
            self.limits.resolve_timeout,
            tls_intercept.dial_plain_any(
                Some(&target.host().to_string()),
                &std::iter::once(dest)
                    .chain(fallbacks.iter().copied())
                    .map(|addr| addr.with_port(target.port().get()))
                    .collect::<Vec<_>>(),
            ),
        )
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()));
        let (mut upstream, hop) = match dialed {
            Ok(dialed) => (
                dialed.stream,
                (dialed.absolute_form, dialed.proxy_authorization, dialed.returns_to),
            ),
            Err(err) => {
                let keep_alive = answer_in_place(
                    client,
                    buffered,
                    &bad_gateway_answer(),
                    answered,
                    Unread::BodyOf {
                        head,
                        continued: continued.load(Ordering::SeqCst),
                    },
                )
                .await;
                return Step::Answered {
                    outcome: IngressOutcome::Proxied {
                        target,
                        dest_ip: dest.reported_ip(),
                        outcome: ProxyOutcome::UpstreamConnectFailed(err),
                    },
                    keep_alive,
                };
            }
        };

        let forwarded = ForwardedHead::from(&parsed, &target, hop.0, hop.1.as_deref(), hop.2.is_some());
        if forwarded.expects_continue
            && !continued.load(Ordering::SeqCst)
            && parsed.framing != Framing::None
            && client.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await.is_err()
        {
            return Step::End(client_gone(target, dest.reported_ip(), 0));
        }
        let mut sent = forwarded.bytes.len() as u64;
        if let Err(err) = send_head(&mut upstream, &forwarded.bytes).await {
            return Step::End(bad_gateway(client, answered, target, dest.reported_ip(), err).await);
        }
        let outgoing = if forwarded.reframed {
            Some(Framing::Chunked)
        } else {
            body_scan
        };
        let relayed_body = match (body_scan, outgoing, &scope) {
            (Some(_), Some(outgoing), Some(scope)) => {
                relay_scanned_body_without_trailers(client, buffered, &mut upstream, parsed.framing, outgoing, scope)
                    .await
            }
            (_, Some(outgoing), _) => {
                relay_rechunked_body_without_trailers(client, buffered, &mut upstream, parsed.framing, outgoing).await
            }
            _ => relay_request_body(client, buffered, &mut upstream, parsed.framing).await,
        };
        let relayed_body = flush_relayed_body(&mut upstream, relayed_body).await;
        match relayed_body {
            Ok(body) => sent += body,
            Err(HttpInterceptError::UpstreamWrite(err)) => {
                return Step::End(bad_gateway(client, answered, target, dest.reported_ip(), err).await);
            }
            Err(error) => {
                return Step::End(refused_or_gone(client, answered, error, target, dest.reported_ip(), sent).await);
            }
        }

        let mut watched = WriteWatch::new(&mut *client);
        let network_strip = response_strip(&tls_intercept.network_options);
        let scrub = match &scope {
            Some(scope) => ResponseScrub::new(Some(scope), network_strip.as_deref(), BodyLimit::DEFAULT),
            None => network_strip
                .as_deref()
                .and_then(|strip| ResponseScrub::network_only(strip, BodyLimit::DEFAULT)),
        };
        match relay_response_hops(
            &mut upstream,
            &mut watched,
            method.as_str(),
            scrub,
            response_hook.as_ref(),
            forwarded.expects_continue,
            HopByHop::Stripped { client: answered },
        )
        .await
        {
            Ok(relayed) => {
                if let Some(key) = hop.2
                    && relayed.upstream == Connection::KeepAlive
                {
                    tls_intercept
                        .parent_pool
                        .give_back(key, upstream, relayed.upstream_hint);
                }
                Step::Next {
                    target,
                    dest_ip: dest.reported_ip(),
                    sent,
                    received: relayed.bytes,
                    keep_alive: relayed.connection == Connection::KeepAlive,
                }
            }
            Err(err) => {
                let responded = watched.wrote();
                let outcome = IngressOutcome::Proxied {
                    target,
                    dest_ip: dest.reported_ip(),
                    outcome: ProxyOutcome::UpstreamConnectFailed(io::Error::other(err.to_string())),
                };
                if responded {
                    return Step::End(outcome);
                }
                let keep_alive =
                    answer_in_place(client, buffered, &bad_gateway_answer(), answered, Unread::Nothing).await;
                Step::Answered { outcome, keep_alive }
            }
        }
    }
}

struct ForwardedHead {
    bytes: Vec<u8>,
    expects_continue: bool,
    /// The body's framing headers went with the hop-by-hop ones, so it is
    /// framed as chunked again.
    reframed: bool,
}

impl ForwardedHead {
    fn from(
        parsed: &ProxiedHead,
        target: &ConnectTarget,
        absolute_form: bool,
        proxy_authorization: Option<&str>,
        keeps_alive: bool,
    ) -> Self {
        let named_by_connection: Vec<String> = parsed
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
            .flat_map(|(_, value)| value.split(','))
            .map(|token| fold_header_name(token.trim()))
            .filter(|token| !token.is_empty())
            .collect();
        let expects_continue = parsed
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("expect") && value.eq_ignore_ascii_case("100-continue"));

        let authority = authority_for(target);
        let request_target = if absolute_form {
            format!("http://{authority}{}", parsed.target)
        } else {
            parsed.target.clone()
        };
        let mut out = format!("{} {request_target} HTTP/1.1\r\n", parsed.method);
        let forwarded: Vec<&(String, String)> = parsed
            .headers
            .iter()
            .filter(|(name, _)| {
                let folded = fold_header_name(name);
                !HOP_BY_HOP.contains(&folded.as_str()) && !named_by_connection.contains(&folded)
            })
            .collect();
        for (name, value) in &forwarded {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        if !forwarded.iter().any(|(name, _)| name.eq_ignore_ascii_case("host")) {
            out.push_str(&format!("host: {authority}\r\n"));
        }
        let framed = forwarded
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        let reframed = parsed.framing != Framing::None && !framed;
        if reframed {
            out.push_str("transfer-encoding: chunked\r\n");
        }
        if let Some(authorization) = proxy_authorization {
            out.push_str(&format!("proxy-authorization: {authorization}\r\n"));
        }
        out.push_str(if keeps_alive {
            "Connection: keep-alive\r\n\r\n"
        } else {
            "Connection: close\r\n\r\n"
        });
        Self {
            bytes: out.into_bytes(),
            expects_continue,
            reframed,
        }
    }
}

async fn refuse<S: ClientIo>(client: &mut S, answered: AnsweredRequest, refusal: RefusalError) -> IngressOutcome {
    let tag = refusal.tag();
    match refusal {
        RefusalError::Unparsed(_) | RefusalError::UnparsedTarget => {
            connect::respond_bad_request(&mut connect::Abrupt(client)).await;
            return IngressOutcome::Rejected(IngressRejectionError::Malformed(tag));
        }
        RefusalError::UnparsedBody(error) => {
            connect::respond_parse_error(&mut connect::Abrupt(client), error).await;
            return IngressOutcome::Rejected(IngressRejectionError::Malformed(tag));
        }
        refusal => {
            let _ = answer_refusal(client, answered, refusal).await;
        }
    }
    drain_unread(client).await;
    IngressOutcome::Rejected(IngressRejectionError::Malformed(tag))
}

async fn bad_gateway<S: ClientIo>(
    client: &mut S,
    answered: AnsweredRequest,
    target: ConnectTarget,
    dest_ip: Option<IpAddr>,
    err: io::Error,
) -> IngressOutcome {
    let _ = client
        .write_all(&bad_gateway_answer().bytes(answered.closing(), SystemTime::now()))
        .await;
    let _ = client.shutdown().await;
    drain_unread(client).await;
    IngressOutcome::Proxied {
        target,
        dest_ip,
        outcome: ProxyOutcome::UpstreamConnectFailed(err),
    }
}

/// Writes `answer` to `request` as the workload's HTTP stack does and reads the rest of
/// the request's body, saying whether the connection goes on to the next
/// request.
async fn answer_in_place<S: ClientIo>(
    client: &mut S,
    buffered: &mut Vec<u8>,
    answer: &FramedAnswer,
    request: AnsweredRequest,
    unread: Unread<'_>,
) -> bool {
    if client
        .write_all(&answer.bytes(request, SystemTime::now()))
        .await
        .is_err()
        || client.flush().await.is_err()
    {
        return false;
    }
    if !answer.keeps_alive(request) {
        return false;
    }
    let framing = match unread {
        Unread::Nothing => return true,
        Unread::BodyOf { head, continued: true } => framing_after_continue(head),
        Unread::BodyOf { head, .. } => framing_of_refused_head(head),
    };
    // A head that got this far has a body framing that can be told.
    #[rustfmt::skip]
    let Some(framing) = framing else { return false };
    drain_body_whole(client, buffered, framing).await
}

/// Refuses a request the body scan or relay refused; any other failure is the client leaving.
async fn refused_or_gone<S: ClientIo>(
    client: &mut S,
    answered: AnsweredRequest,
    error: HttpInterceptError,
    target: ConnectTarget,
    dest_ip: Option<IpAddr>,
    sent: u64,
) -> IngressOutcome {
    if let HttpInterceptError::Refused(refusal) = error {
        return refuse(client, answered, refusal).await;
    }
    client_gone(target, dest_ip, sent)
}

fn client_gone(target: ConnectTarget, dest_ip: Option<IpAddr>, sent: u64) -> IngressOutcome {
    IngressOutcome::Proxied {
        target,
        dest_ip,
        outcome: ProxyOutcome::Completed {
            bytes_client_to_upstream: sent,
            bytes_upstream_to_client: 0,
        },
    }
}

/// Reads what the client is still sending after it was answered, so closing
/// the connection does not reset it before the client has read the answer.
async fn drain_unread<C: AsyncRead + Unpin>(client: &mut C) {
    let drain = async {
        let mut sink = [0u8; 8192];
        let mut total = 0usize;
        while total < MAX_DRAIN_BYTES {
            match client.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(n) => total += n,
            }
        }
    };
    let _ = timeout(DRAIN_TIMEOUT, drain).await;
}

fn refused_request(
    target: &ConnectTarget,
    method: &HttpMethod,
    request_target: &str,
    denial: &RequestDenial,
) -> RefusedRequest {
    RefusedRequest::new(
        method.as_str(),
        &format!("http://{}{request_target}", authority_for(target)),
        denial.denial_reason(),
    )
}

fn authority_for(target: &ConnectTarget) -> String {
    match target.port().get() {
        80 => target.host().to_string(),
        port => format!("{}:{port}", target.host()),
    }
}

/// Writes the head to a parent proxy. A parent proxy over TLS may keep it
/// buffered, and nothing else writes to it until the response is awaited.
async fn send_head<U: AsyncWrite + Unpin>(upstream: &mut U, head: &[u8]) -> io::Result<()> {
    upstream.write_all(head).await?;
    upstream.flush().await
}

/// The body's last record may still be buffered on a TLS parent proxy.
async fn flush_relayed_body<U: AsyncWrite + Unpin>(
    upstream: &mut U,
    relayed: Result<u64, HttpInterceptError>,
) -> Result<u64, HttpInterceptError> {
    match relayed {
        Ok(body) => upstream
            .flush()
            .await
            .map(|()| body)
            .map_err(HttpInterceptError::UpstreamWrite),
        other => other,
    }
}

#[cfg(test)]
mod tests;
