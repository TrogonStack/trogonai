//! What the proxy does for one authenticated connection (ADR#0085 Decisions
//! 2, 5 and 7).
//!
//! The order is the security argument:
//!
//! 1. read one bounded CONNECT head, nothing else is served;
//! 2. `admit` the caller and the exact destination, before any DNS or dial;
//! 3. resolve once, keep only addresses the policy intended, dial one of them;
//! 4. answer `200`, then check that the first TLS record names the CONNECT
//!    host, and only then forward bytes.
//!
//! A tunnel is audited twice at most. The permit is recorded after the dial
//! succeeds and before `200`, and is acknowledged first: a proxy that cannot
//! record a permit refuses with `AUDIT_UNAVAILABLE` and forwards nothing. Every
//! refusal is recorded too, but the refusal is sent whether or not the record
//! is written. A client that opens a tunnel and then sends the wrong SNI has
//! two records: the permit that opened it and the refusal that closed it. A
//! `AUDIT_UNAVAILABLE` refusal is the one answer not recorded, because it is an
//! operational fault of the sink itself.

use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::TcpStream;
use tokio::time::{MissedTickBehavior, interval, sleep_until, timeout};
use trogon_std::{EpochClock, SystemClock};

use crate::admission::{Admission, admit};
use crate::agent_id::AgentId;
use crate::audit::{AuditEvent, AuditSink, Decision};
use crate::audit_error::AuditError;
use crate::connect_request::{Request, RequestError, read_request};
use crate::constants::CONNECTION_ESTABLISHED;
use crate::destination::Destination;
use crate::destination_host::DestinationHost;
use crate::dial_targets::dial_targets;
use crate::fault::Fault;
use crate::first_record::read_first_record;
use crate::host_name::HostName;
use crate::limits::Limits;
use crate::peer_identity::PeerIdentity;
use crate::policy_snapshot::PolicySnapshot;
use crate::resolver::Resolver;

const POLICY_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
const DENIAL_AUDIT_TIMEOUT: Duration = Duration::from_secs(1);

struct EndedAuthority {
    destination: Option<Destination>,
    address: Option<IpAddr>,
    fault: Fault,
}

/// Everything a connection needs besides the connection itself.
pub struct Proxy<C = SystemClock> {
    clock: C,
    snapshot: Arc<dyn PolicySnapshot>,
    resolver: Arc<dyn Resolver>,
    audit: Arc<dyn AuditSink>,
    limits: Limits,
    non_tls_ports: HashSet<NonZeroU16>,
}

impl Proxy<SystemClock> {
    /// `non_tls_ports` are destination ports whose tunnels are not TLS, so the
    /// SNI check is skipped for them.
    pub fn new(
        snapshot: Arc<dyn PolicySnapshot>,
        resolver: Arc<dyn Resolver>,
        audit: Arc<dyn AuditSink>,
        limits: Limits,
        non_tls_ports: HashSet<NonZeroU16>,
    ) -> Self {
        Self {
            clock: SystemClock,
            snapshot,
            resolver,
            audit,
            limits,
            non_tls_ports,
        }
    }
}

impl<Clock: EpochClock> Proxy<Clock> {
    pub fn with_clock<D: EpochClock>(self, clock: D) -> Proxy<D> {
        Proxy {
            clock,
            snapshot: self.snapshot,
            resolver: self.resolver,
            audit: self.audit,
            limits: self.limits,
            non_tls_ports: self.non_tls_ports,
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub(crate) fn audit(&self) -> Arc<dyn AuditSink> {
        Arc::clone(&self.audit)
    }

    /// Serves one connection with the exact peer identity verified by its transport.
    pub async fn serve<C>(&self, mut client: C, peer: &PeerIdentity)
    where
        C: AsyncRead + AsyncWrite + Unpin,
    {
        let ended = if peer.is_current(self.clock.system_time()) {
            tokio::select! {
                biased;
                () = sleep_until(peer.deadline()) => Some(EndedAuthority {
                    destination: None,
                    address: None,
                    fault: Fault::AuthorityExpired,
                }),
                outcome = self.run(&mut client, peer) => match outcome {
                    Ok(ended) => ended,
                    Err(error) => {
                        tracing::debug!(agent = %peer.agent(), error = %error, "connection ended with an i/o error");
                        None
                    }
                }
            }
        } else {
            Some(EndedAuthority {
                destination: None,
                address: None,
                fault: Fault::AuthorityExpired,
            })
        };
        if let Some(ended) = ended {
            drop(client);
            let _ = timeout(
                DENIAL_AUDIT_TIMEOUT,
                self.record(
                    peer.agent(),
                    ended.destination.as_ref(),
                    ended.address,
                    Decision::Refused(ended.fault),
                ),
            )
            .await;
        } else {
            let _ = tokio::time::timeout_at(peer.deadline(), client.shutdown()).await;
        }
    }

    async fn run<C>(&self, client: &mut C, peer: &PeerIdentity) -> io::Result<Option<EndedAuthority>>
    where
        C: AsyncRead + AsyncWrite + Unpin,
    {
        let agent = peer.agent();
        let head = timeout(
            self.limits.head_timeout,
            read_request(client, self.limits.max_head_bytes),
        )
        .await;
        let (request, leftover) = match head {
            Err(_) => {
                tracing::debug!(%agent, "request head timed out");
                return Ok(None);
            }
            Ok(Err(RequestError::Closed)) => return Ok(None),
            Ok(Err(RequestError::Io(error))) => return Err(error),
            Ok(Err(error)) => {
                tracing::debug!(%agent, error = %error, "request refused");
                return self.refuse(client, agent, None, None, Fault::BadRequest).await;
            }
            Ok(Ok(parsed)) => parsed,
        };
        let destination = match request {
            Request::Connect(destination) => destination,
            Request::OtherMethod => {
                return self.refuse(client, agent, None, None, Fault::MethodNotAllowed).await;
            }
        };

        if let Err(fault) = self.check_request_authority(peer, &destination) {
            return self.refuse(client, agent, Some(&destination), None, fault).await;
        }

        let candidates = match self.candidates(&destination).await {
            Some(candidates) => candidates,
            None => {
                return self
                    .refuse(client, agent, Some(&destination), None, Fault::UpstreamUnreachable)
                    .await;
            }
        };
        if let Err(fault) = self.check_request_authority(peer, &destination) {
            return self.refuse(client, agent, Some(&destination), None, fault).await;
        }
        let targets = match dial_targets(
            self.snapshot.as_ref(),
            agent,
            destination.port(),
            &candidates,
            self.clock.system_time(),
        ) {
            Ok(targets) => targets,
            Err(fault) => return self.refuse(client, agent, Some(&destination), None, fault).await,
        };
        let dialled = match self.dial(peer, &destination, &targets).await {
            Ok(dialled) => dialled,
            Err(fault) => return self.refuse(client, agent, Some(&destination), None, fault).await,
        };
        let Some((mut upstream, address)) = dialled else {
            return self
                .refuse(client, agent, Some(&destination), None, Fault::UpstreamUnreachable)
                .await;
        };

        let resolved = Some(address.ip());
        if self
            .record(agent, Some(&destination), resolved, Decision::Permitted)
            .await
            .is_err()
        {
            drop(upstream);
            return self
                .refuse(client, agent, Some(&destination), resolved, Fault::AuditUnavailable)
                .await;
        }

        if let Err(fault) = self.check_authority(peer, &destination, address) {
            drop(upstream);
            return self.refuse(client, agent, Some(&destination), resolved, fault).await;
        }

        let tunnel = async {
            client.write_all(CONNECTION_ESTABLISHED).await?;
            client.flush().await?;

            let mut pending = leftover;
            if let Some(expected) = self.expected_sni(&destination) {
                match read_first_record(client, pending, self.limits.first_record_timeout).await {
                    Ok((requested, record)) if requested == *expected => pending = record,
                    outcome => {
                        if let Err(error) = &outcome {
                            tracing::debug!(%agent, error = %error, "first record rejected");
                        }
                        return io::Result::Ok(Some(Fault::SniMismatch));
                    }
                }
            }
            if let Err(fault) = self.check_authority(peer, &destination, address) {
                return Ok(Some(fault));
            }
            upstream.write_all(&pending).await?;
            copy_bidirectional(client, &mut upstream).await?;
            Ok(None)
        };
        let fault = tokio::select! {
            biased;
            fault = self.monitor_authority(peer, &destination, address) => Some(fault),
            result = tunnel => result?,
        };
        Ok(fault.map(|fault| EndedAuthority {
            destination: Some(destination),
            address: resolved,
            fault,
        }))
    }

    fn check_request_authority(&self, peer: &PeerIdentity, destination: &Destination) -> Result<(), Fault> {
        let now = self.clock.system_time();
        if !peer.is_current(now) {
            return Err(Fault::AuthorityExpired);
        }
        if let Admission::Refuse(fault) = admit(self.snapshot.as_ref(), peer.agent(), destination, now) {
            return Err(fault);
        }
        Ok(())
    }

    fn check_authority(
        &self,
        peer: &PeerIdentity,
        destination: &Destination,
        address: SocketAddr,
    ) -> Result<(), Fault> {
        self.check_request_authority(peer, destination)?;
        let now = self.clock.system_time();
        dial_targets(
            self.snapshot.as_ref(),
            peer.agent(),
            destination.port(),
            &[address.ip()],
            now,
        )?;
        Ok(())
    }

    async fn monitor_authority(&self, peer: &PeerIdentity, destination: &Destination, address: SocketAddr) -> Fault {
        let mut checks = interval(POLICY_RECHECK_INTERVAL);
        checks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            checks.tick().await;
            if let Err(fault) = self.check_authority(peer, destination, address) {
                return fault;
            }
        }
    }

    /// Resolves a name once; an IP literal is its own only candidate.
    async fn candidates(&self, destination: &Destination) -> Option<Vec<IpAddr>> {
        match destination.host() {
            DestinationHost::Ip(address) => Some(vec![*address]),
            DestinationHost::Name(name) => {
                match timeout(self.limits.connect_timeout, self.resolver.resolve(name)).await {
                    Ok(Ok(addresses)) if !addresses.is_empty() => Some(addresses),
                    Ok(Ok(_)) => None,
                    Ok(Err(error)) => {
                        tracing::debug!(error = %error, "name resolution failed");
                        None
                    }
                    Err(_) => {
                        tracing::debug!("name resolution timed out");
                        None
                    }
                }
            }
        }
    }

    /// Tries the validated addresses in order; nothing else is ever dialed.
    async fn dial(
        &self,
        peer: &PeerIdentity,
        destination: &Destination,
        targets: &[SocketAddr],
    ) -> Result<Option<(TcpStream, SocketAddr)>, Fault> {
        for target in targets {
            self.check_authority(peer, destination, *target)?;
            let attempt = timeout(self.limits.connect_timeout, TcpStream::connect(target))
                .await
                .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()));
            match attempt {
                Ok(stream) => {
                    self.check_authority(peer, destination, *target)?;
                    return Ok(Some((stream, *target)));
                }
                Err(error) => tracing::debug!(error = %error, "upstream connect failed"),
            }
        }
        Ok(None)
    }

    /// The name the first TLS record must carry. An IP literal has no name to
    /// compare with and its exact permit is the whole boundary, so only named
    /// destinations on TLS ports are checked.
    fn expected_sni<'a>(&self, destination: &'a Destination) -> Option<&'a HostName> {
        match destination.host() {
            DestinationHost::Name(name) if !self.non_tls_ports.contains(&destination.port()) => Some(name),
            _ => None,
        }
    }

    async fn refuse<C>(
        &self,
        client: &mut C,
        agent: &AgentId,
        destination: Option<&Destination>,
        resolved: Option<IpAddr>,
        fault: Fault,
    ) -> io::Result<Option<EndedAuthority>>
    where
        C: AsyncWrite + Unpin,
    {
        if fault != Fault::AuditUnavailable {
            let _ = self
                .record(agent, destination, resolved, Decision::Refused(fault))
                .await;
        }
        client.write_all(&fault.response()).await?;
        client.flush().await.map(|()| None)
    }

    async fn record(
        &self,
        agent: &AgentId,
        destination: Option<&Destination>,
        resolved: Option<IpAddr>,
        decision: Decision,
    ) -> Result<(), AuditError> {
        self.audit
            .record(&AuditEvent {
                agent,
                destination,
                resolved_address: resolved,
                decision,
                snapshot_source: self.snapshot.source(),
            })
            .await
    }
}

#[cfg(test)]
mod tests;
