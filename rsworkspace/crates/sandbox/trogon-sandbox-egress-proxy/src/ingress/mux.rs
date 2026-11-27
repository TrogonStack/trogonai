//! One listener for HTTP and SOCKS5: the first byte picks the protocol, the
//! token gates it, and the destination goes through the typed policy before
//! anything is dialed.

use crate::host_kind::{HostClassification, HostKind};
use std::io;
use std::net::IpAddr;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, timeout};
use trogon_sandbox_contract::Host;
use trogon_sandbox_contract::{DecideMethod, DomainRuleAction, EgressViolationReason, NetworkPolicy, RefusedRequest};
use trogon_sandbox_contract::{DenyReason, Port};

use super::admission::Admission;
use super::connect::{self, HttpRequest};
use super::denied::denied;
use super::ingress_core::Ingress;
use super::socks5::{self, Reply};
use super::unreachable::unreachable;
use super::{ClientIo, ConnectTarget, IngressOutcome, IngressRejectionError};
use crate::deny_explanation::{deny_explanation, opaque_tunnels};
use crate::dial::{LegAddresses, LegVetting};
use crate::opaque_tunnel::{self, would_splice_unread};
use crate::proxy::{Denial, ProxyOutcome, TlsInterceptContext, forward_decided, interception_for, splices_opaquely};
use crate::resolve::{VettedAddr, VettedEndpoint};
use crate::tls_intercept::PrefixedStream;
use crate::tunnel_deadlines::TunnelDeadlines;
use crate::tunnel_gate::TunnelSlot;

mod constants;
use constants::{MAX_DECISION_CAPTURE_BYTES, SOCKS_HEADER_BYTES, SOCKS4_VERSION_BYTE, SOCKS5_VERSION_BYTE};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Protocol {
    Http,
    Socks4,
    Socks5,
}

impl Protocol {
    pub fn from_first_byte(byte: u8) -> Option<Self> {
        match byte {
            SOCKS4_VERSION_BYTE => Some(Self::Socks4),
            SOCKS5_VERSION_BYTE => Some(Self::Socks5),
            b'A'..=b'Z' | b'\r' | b'\n' => Some(Self::Http),
            _ => None,
        }
    }
}

impl Ingress {
    /// A refusal at the cap is reported once, until the tunnels in use fall to
    /// half the cap, as the proxy reports it.
    fn tunnel_limit_reached(&self, target: ConnectTarget, cap: u32) -> IngressOutcome {
        if !self.gate.first_refusal(self.audience) {
            return IngressOutcome::Rejected(IngressRejectionError::TunnelLimitReached);
        }
        let reason = format!(
            "the sandbox's limit of {} TLS-terminated tunnels at once is reached; the limit is network.tlsTerminate.maxTunnels in the sandbox's configuration",
            cap
        );
        let denial = Denial::new(
            Some(target.host().to_string()),
            EgressViolationReason::TunnelLimitReached,
        )
        .refusing(RefusedRequest::new("CONNECT", &target.to_string(), &reason));
        IngressOutcome::Proxied {
            target,
            dest_ip: None,
            outcome: ProxyOutcome::Denied(denial),
        }
    }

    /// The deadlines a terminated tunnel runs under: the gate's handshake
    /// timeout, when set, replaces the configured one, and the window opens at
    /// the CONNECT.
    fn deadlines_from(&self, connected: Instant, tls_intercept: &TlsInterceptContext) -> TunnelDeadlines {
        let deadlines = tls_intercept.tunnel_deadlines();
        TunnelDeadlines {
            handshake: self.gate.handshake().unwrap_or(deadlines.handshake),
            ..deadlines
        }
        .counting_handshake_from(connected)
    }

    /// The slot a tunnel about to be terminated takes. Tunnels that are not
    /// terminated take none.
    fn terminated_slot(&self) -> Result<TunnelSlot, u32> {
        self.gate.take()
    }

    pub async fn serve<S: ClientIo>(
        &self,
        client: S,
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        let (protocol, client) = match self.first_protocol(client).await {
            Ok(parts) => parts,
            Err(rejection) => return IngressOutcome::Rejected(rejection),
        };
        match protocol {
            Protocol::Http => self.serve_http(client, policy, tls_intercept).await,
            Protocol::Socks4 | Protocol::Socks5 if self.token.is_none() && !self.open_socks => {
                let mut client = client;
                connect::respond_bad_request(&mut client).await;
                IngressOutcome::Rejected(IngressRejectionError::UnknownProtocol)
            }
            Protocol::Socks4 => self.reject_socks4(client).await,
            Protocol::Socks5 => self.serve_socks5(client, policy, tls_intercept).await,
        }
    }

    async fn first_protocol<S: ClientIo>(
        &self,
        mut client: S,
    ) -> Result<(Protocol, PrefixedStream<S>), IngressRejectionError> {
        let mut first = [0u8; 1];
        timeout(self.limits.handshake_timeout, client.read_exact(&mut first))
            .await
            .map_err(|_| IngressRejectionError::HandshakeTimedOut)??;
        let Some(protocol) = Protocol::from_first_byte(first[0]) else {
            connect::respond_bad_request(&mut client).await;
            return Err(IngressRejectionError::UnknownProtocol);
        };
        Ok((protocol, PrefixedStream::new(client, first.to_vec())))
    }

    async fn serve_http<S: ClientIo>(
        &self,
        mut client: PrefixedStream<S>,
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        let request = match connect::read_request_timed(
            &mut client,
            self.token.as_ref(),
            self.limits.max_head_bytes,
            self.crl.as_deref(),
            self.limits.ascii_hosts_only,
            self.options.reasoned_refusals.is_enabled(),
            Some(connect::RequestTiming {
                head: self.limits.head_timeout,
                idle_wait: self.idle_wait(),
                advertised: self.limits.keep_alive_timeout,
            }),
        )
        .await
        {
            Ok(request) => request,
            Err(rejection) => return IngressOutcome::Rejected(rejection),
        };

        match request {
            HttpRequest::Connect {
                target,
                mut early_bytes,
            } => {
                let (admission, mut client) = match self
                    .admit_watching(
                        client,
                        &mut early_bytes,
                        tls_intercept.decision_window_capture,
                        policy,
                        &target,
                    )
                    .await
                {
                    Ok(admitted) => admitted,
                    Err(gone) => return IngressOutcome::Rejected(IngressRejectionError::Io(gone)),
                };
                match admission {
                    Admission::Denied {
                        dest_ip,
                        reason,
                        explanation,
                    } => {
                        tls_intercept.explain_denial(target.host(), target.port().get(), explanation.as_ref());
                        match connect::address_denial_words(&reason, explanation.as_ref()) {
                            Some(words) => {
                                connect::respond_address_denied(&mut client, target.host(), &words).await;
                            }
                            None => {
                                let phrase = connect::DenyPhrase::for_refusal(
                                    self.options.reasoned_refusals.is_enabled(),
                                    &reason,
                                    explanation.as_ref(),
                                );
                                connect::respond_blocked(&mut client, &phrase).await
                            }
                        }
                        denied(target, dest_ip, reason)
                    }
                    Admission::Unresolved(err) => {
                        connect::respond_unreachable(&mut client).await;
                        unreachable(target, err)
                    }
                    Admission::Allowed {
                        dest,
                        fallbacks,
                        action,
                    } => {
                        if refuses_opaque(policy, &target, action.as_ref(), tls_intercept) {
                            connect::respond_opaque_refused(&mut client).await;
                            return opaque_refused(target, dest.reported_ip());
                        }
                        if relays_opaquely(&target, action.as_ref(), tls_intercept) {
                            return self
                                .tunnel_opaquely(client, early_bytes, target, dest, &fallbacks, tls_intercept)
                                .await;
                        }
                        let connected = Instant::now();
                        let terminated = match self.terminated_slot() {
                            Ok(slot) => slot,
                            Err(cap) => {
                                connect::respond_tunnel_cap(&mut client).await;
                                return self.tunnel_limit_reached(target, cap);
                            }
                        };
                        if let Err(err) = connect::respond_established(&mut client).await {
                            return IngressOutcome::Rejected(IngressRejectionError::Io(err));
                        }
                        let deadlines = self.deadlines_from(connected, tls_intercept);
                        let prefix =
                            opaque_tunnel::read_first_bytes(&mut client, early_bytes, deadlines.handshake_left()).await;
                        if opaque_tunnel::judged_not_tls(&prefix) {
                            drop(terminated);
                            if opaque_tunnels(policy).refuses() {
                                return opaque_refused(target, dest.reported_ip());
                            }
                            return self
                                .splice_unread(client, prefix, target, dest, &fallbacks, tls_intercept)
                                .await;
                        }
                        self.forward(
                            client,
                            prefix,
                            target,
                            dest,
                            &fallbacks,
                            action,
                            policy,
                            tls_intercept,
                            deadlines,
                        )
                        .await
                    }
                }
            }
            HttpRequest::MalformedConnect { host, port } => {
                let phrase = connect::DenyPhrase::malformed_host(self.options.reasoned_refusals.is_enabled());
                connect::respond_blocked(&mut client, &phrase).await;
                IngressOutcome::Rejected(IngressRejectionError::MalformedHost { host, port })
            }
            HttpRequest::CertificateRevocationList => IngressOutcome::CrlServed,
            HttpRequest::Plain { head, leftover } => {
                self.serve_plain(client, policy, tls_intercept, head, leftover).await
            }
        }
    }

    /// Closes a SOCKS4 client once its greeting header has arrived, reading
    /// what came with it, so the close is not a reset.
    async fn reject_socks4<S: ClientIo>(&self, mut client: PrefixedStream<S>) -> IngressOutcome {
        let read_header = async {
            let mut chunk = [0u8; 8192];
            let mut seen = 0usize;
            while seen < SOCKS_HEADER_BYTES {
                match client.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen += n,
                }
            }
        };
        let _ = timeout(self.limits.handshake_timeout, read_header).await;
        IngressOutcome::Rejected(IngressRejectionError::UnsupportedSocksVersion)
    }

    async fn serve_socks5<S: ClientIo>(
        &self,
        mut client: PrefixedStream<S>,
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        let handshake = match &self.token {
            Some(token) => timeout(self.limits.handshake_timeout, socks5::handshake(&mut client, token)).await,
            None => {
                timeout(self.limits.handshake_timeout, async {
                    socks5::handshake_open(&mut client).await.map(socks5::Handshake::Target)
                })
                .await
            }
        };
        let target = match handshake {
            Ok(Ok(socks5::Handshake::Target(target))) => target,
            Ok(Ok(socks5::Handshake::Unauthenticated(connect))) => {
                return self
                    .refuse_unauthenticated_socks5(client, connect, policy, tls_intercept)
                    .await;
            }
            Ok(Err(rejection)) => return IngressOutcome::Rejected(rejection),
            Err(_) => return IngressOutcome::Rejected(IngressRejectionError::HandshakeTimedOut),
        };

        if opaque_tunnels(policy).refuses() {
            socks5::reply(&mut client, Reply::GeneralFailure).await;
            return socks_tunnel_refused(target);
        }

        match self.admit(policy, &target, DecideMethod::Socks5).await {
            Admission::Denied {
                dest_ip,
                reason,
                explanation,
            } => {
                tls_intercept.explain_denial(target.host(), target.port().get(), explanation.as_ref());
                socks5::reply(&mut client, Reply::NotAllowed).await;
                denied(target, dest_ip, reason)
            }
            Admission::Unresolved(err) => {
                socks5::reply(&mut client, Reply::HostUnreachable).await;
                unreachable(target, err)
            }
            Admission::Allowed { dest, fallbacks, .. } => {
                self.tunnel_socks5(client, target, dest, &fallbacks, tls_intercept)
                    .await
            }
        }
    }

    /// A client that cannot authenticate is never tunnelled. An explicit deny
    /// rule that matches its destination names the reason in the refusal and
    /// is recorded; a host that is merely off the list gets the generic one.
    async fn refuse_unauthenticated_socks5<S: ClientIo>(
        &self,
        mut client: PrefixedStream<S>,
        connect: socks5::UnauthenticatedConnect,
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        if !socks5::is_valid_host(connect.host()) {
            socks5::reply(&mut client, Reply::NotAllowed).await;
            return IngressOutcome::Rejected(IngressRejectionError::MalformedHost {
                host: connect.host().to_string(),
                port: connect.port(),
            });
        }
        let target = Host::new(connect.host())
            .ok()
            .zip(Port::new(connect.port()).ok())
            .map(|(host, port)| ConnectTarget::new(host, port));
        let explanation = target
            .as_ref()
            .and_then(|target| deny_explanation(&self.options, policy, target.host(), target.port().get()));
        socks5::refuse_unauthenticated(
            &mut client,
            connect.port(),
            explanation.as_ref().map(DenyReason::as_str),
        )
        .await;
        match (target, explanation) {
            (Some(target), Some(reason)) => {
                tls_intercept.explain_denial(target.host(), target.port().get(), Some(&reason));
                denied(target, None, EgressViolationReason::PolicyDenied)
            }
            _ => IngressOutcome::Rejected(IngressRejectionError::Unauthenticated),
        }
    }

    /// A SOCKS tunnel is never terminated: it is carried unread to the
    /// destination, whatever the domain rule says.
    async fn tunnel_socks5<S: ClientIo>(
        &self,
        mut client: PrefixedStream<S>,
        target: ConnectTarget,
        dest: VettedAddr,
        fallbacks: &[VettedAddr],
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        let endpoints = endpoints_for(&target, dest, fallbacks);
        match tls_intercept
            .dial_routed_any(Some(&target.host().to_string()), true, &endpoints)
            .await
        {
            Ok(upstream) => {
                socks5::reply(&mut client, Reply::Succeeded).await;
                IngressOutcome::Proxied {
                    outcome: splice(client, upstream).await,
                    target,
                    dest_ip: dest.reported_ip(),
                }
            }
            Err(err) => {
                let code = if err.kind() == io::ErrorKind::ConnectionRefused {
                    Reply::ConnectionRefused
                } else {
                    Reply::HostUnreachable
                };
                socks5::reply(&mut client, code).await;
                unreachable(target, err)
            }
        }
    }

    /// Dials first and answers `200` only once the upstream is open, so a
    /// destination that cannot be reached gets a `502` instead of a tunnel
    /// that closes at once.
    async fn tunnel_opaquely<S: ClientIo>(
        &self,
        mut client: S,
        early_bytes: Vec<u8>,
        target: ConnectTarget,
        dest: VettedAddr,
        fallbacks: &[VettedAddr],
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        let endpoints = endpoints_for(&target, dest, fallbacks);
        let name = target.host().to_string();
        let upstream = match tls_intercept.dial_routed_any(Some(&name), true, &endpoints).await {
            Ok(upstream) => upstream,
            Err(err) => {
                connect::respond_unreachable(&mut client).await;
                return IngressOutcome::Proxied {
                    target,
                    dest_ip: dest.reported_ip(),
                    outcome: ProxyOutcome::UpstreamConnectFailed(err),
                };
            }
        };
        if let Err(err) = connect::respond_established(&mut client).await {
            return IngressOutcome::Rejected(IngressRejectionError::Io(err));
        }
        let outcome = splice_after(client, upstream, &early_bytes).await;
        IngressOutcome::Proxied {
            target,
            dest_ip: dest.reported_ip(),
            outcome,
        }
    }

    /// Carries a tunnel the proxy began to terminate whose first bytes turned
    /// out not to be TLS, once `200` has been sent.
    async fn splice_unread<S: ClientIo>(
        &self,
        client: S,
        prefix: Vec<u8>,
        target: ConnectTarget,
        dest: VettedAddr,
        fallbacks: &[VettedAddr],
        tls_intercept: &TlsInterceptContext,
    ) -> IngressOutcome {
        let endpoints = endpoints_for(&target, dest, fallbacks);
        let name = target.host().to_string();
        let outcome = match tls_intercept.dial_routed_any(Some(&name), true, &endpoints).await {
            Ok(upstream) => splice_after(client, upstream, &prefix).await,
            Err(err) => ProxyOutcome::UpstreamConnectFailed(err),
        };
        IngressOutcome::Proxied {
            target,
            dest_ip: dest.reported_ip(),
            outcome,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn forward<S: ClientIo>(
        &self,
        client: S,
        prefix: Vec<u8>,
        target: ConnectTarget,
        dest: VettedAddr,
        fallbacks: &[VettedAddr],
        action: Option<DomainRuleAction>,
        policy: &NetworkPolicy,
        tls_intercept: &TlsInterceptContext,
        deadlines: TunnelDeadlines,
    ) -> IngressOutcome {
        let port = target.port().get();
        let vetting = match target.host().kind() {
            HostKind::Domain(_) => LegVetting::Probe,
            HostKind::Ip(_) => LegVetting::AsGiven,
        };
        let upstream = LegAddresses::new(
            dest.with_port(port),
            fallbacks.iter().map(|addr| addr.with_port(port)).collect(),
            vetting,
        );
        let sni = match target.host().kind() {
            HostKind::Domain(name) => Some(name.as_str().to_string()),
            HostKind::Ip(ip) => tls_intercept.terminates_every_allowed_host().then(|| ip.to_string()),
        };
        let interception = interception_for(
            policy,
            tls_intercept,
            &self.resolver,
            sni.as_deref(),
            dest.ip(),
            port,
            matches!(target.host().kind(), HostKind::Domain(_)),
        )
        .await;
        let outcome = forward_decided(
            client,
            prefix,
            sni.as_deref(),
            action,
            upstream,
            tls_intercept,
            opaque_tunnels(policy),
            tls_intercept.request_checks(policy),
            deadlines,
            interception,
        )
        .await;
        IngressOutcome::Proxied {
            target,
            dest_ip: dest.reported_ip(),
            outcome,
        }
    }

    /// Decides the destination while reading what the client sends meanwhile,
    /// when it holds no terminating CA: the bytes join the
    /// tunnel's first flight, and a client that closes or sends more than
    /// the cap before the verdict is gone, with its verdict dropped
    ///. A client already
    /// gone when the decision would begin spends none; one gone later is
    /// closed at once while the decision runs on, so its work still serves a
    /// retry.
    async fn admit_watching<C: ClientIo>(
        &self,
        mut client: C,
        early_bytes: &mut Vec<u8>,
        watching: bool,
        policy: &NetworkPolicy,
        target: &ConnectTarget,
    ) -> Result<(Admission, C), io::Error> {
        if !watching {
            let admission = self.admit(policy, target, DecideMethod::Connect).await;
            return Ok((admission, client));
        }
        let mut capture = DecisionCapture::default();
        let mut chunk = [0u8; 8192];
        // One turn of the reactor first, so an end the socket already carries
        // is seen before a decision is begun.
        tokio::task::yield_now().await;
        let already = tokio::select! {
            biased;
            read = client.read(&mut chunk) => Some(read),
            () = std::future::ready(()) => None,
        };
        let gone_before = already.is_some_and(|read| !capture.take(read, &chunk, early_bytes));
        if !gone_before {
            let decision = self.admit(policy, target, DecideMethod::Connect);
            tokio::pin!(decision);
            loop {
                tokio::select! {
                    biased;
                    read = client.read(&mut chunk) => {
                        if !capture.take(read, &chunk, early_bytes) {
                            break;
                        }
                    }
                    admission = &mut decision => return Ok((admission, client)),
                }
            }
            drop(client);
            decision.await;
        }
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the client left before the destination was decided",
        ))
    }
}

/// What a CONNECT client sent while its destination was decided.
#[derive(Default)]
struct DecisionCapture {
    bytes: usize,
}

impl DecisionCapture {
    /// Keeps one read's bytes for the tunnel's first flight. False once the
    /// client has ended, failed or sent more than the cap.
    fn take(&mut self, read: io::Result<usize>, chunk: &[u8], early_bytes: &mut Vec<u8>) -> bool {
        match read {
            Ok(n) if n > 0 && self.bytes + n <= MAX_DECISION_CAPTURE_BYTES => {
                self.bytes += n;
                early_bytes.extend_from_slice(&chunk[..n]);
                true
            }
            _ => false,
        }
    }
}

fn relays_opaquely(
    target: &ConnectTarget,
    action: Option<&DomainRuleAction>,
    tls_intercept: &TlsInterceptContext,
) -> bool {
    let sni = match target.host().kind() {
        HostKind::Domain(name) => Some(name.as_str()),
        HostKind::Ip(_) => None,
    };
    if tls_intercept.terminates_target(target.host(), target.port().get()) {
        return false;
    }
    splices_opaquely(
        sni,
        action,
        target.port().get(),
        tls_intercept.secrets_for(sni, target.port().get()),
    )
}

fn endpoints_for(target: &ConnectTarget, dest: VettedAddr, fallbacks: &[VettedAddr]) -> Vec<VettedEndpoint> {
    let port = target.port().get();
    std::iter::once(dest)
        .chain(fallbacks.iter().copied())
        .map(|addr| addr.with_port(port))
        .collect()
}

fn refuses_opaque(
    policy: &NetworkPolicy,
    target: &ConnectTarget,
    action: Option<&DomainRuleAction>,
    tls_intercept: &TlsInterceptContext,
) -> bool {
    let terminated_by_secrets = tls_intercept.terminates_target(target.host(), target.port().get())
        || match target.host().kind() {
            HostKind::Domain(name) => tls_intercept
                .secrets_for(Some(name.as_str()), target.port().get())
                .is_some_and(|secrets| secrets.covers(name.as_str(), target.port().get())),
            HostKind::Ip(_) => false,
        };
    opaque_tunnels(policy).refuses() && would_splice_unread(action, terminated_by_secrets)
}

fn opaque_refused(target: ConnectTarget, dest_ip: Option<IpAddr>) -> IngressOutcome {
    let denial = opaque_tunnel::refusal(Some(&target.host().to_string()));
    IngressOutcome::Proxied {
        target,
        dest_ip,
        outcome: ProxyOutcome::Denied(denial),
    }
}

/// Writes `lead` to the upstream ahead of the splice. The splice only flushes
/// what it relays, so a TLS record left buffered here would never reach the
/// upstream.
async fn splice_after<S: ClientIo, U: AsyncRead + AsyncWrite + Unpin>(
    client: S,
    mut upstream: U,
    lead: &[u8],
) -> ProxyOutcome {
    let led = async {
        upstream.write_all(lead).await?;
        upstream.flush().await
    };
    match led.await {
        Ok(()) => splice(client, upstream).await,
        Err(err) => ProxyOutcome::UpstreamConnectFailed(err),
    }
}

async fn splice<S: ClientIo, U: AsyncRead + AsyncWrite + Unpin>(mut client: S, mut upstream: U) -> ProxyOutcome {
    match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
        Ok((bytes_client_to_upstream, bytes_upstream_to_client)) => ProxyOutcome::Completed {
            bytes_client_to_upstream,
            bytes_upstream_to_client,
        },
        Err(err) => ProxyOutcome::UpstreamConnectFailed(err),
    }
}

fn socks_tunnel_refused(target: ConnectTarget) -> IngressOutcome {
    let request = RefusedRequest::new("SOCKS", &target.to_string(), "refuseOpaqueTunnels");
    let denial = opaque_tunnel::refusal(Some(&target.host().to_string())).refusing(request);
    IngressOutcome::Proxied {
        dest_ip: None,
        outcome: ProxyOutcome::Denied(denial),
        target,
    }
}

#[cfg(test)]
mod tests;
