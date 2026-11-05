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
use std::time::SystemTime;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::TcpStream;
use tokio::time::timeout;

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
use crate::policy_snapshot::PolicySnapshot;
use crate::resolver::Resolver;

/// Everything a connection needs besides the connection itself.
pub struct Proxy {
    snapshot: Arc<dyn PolicySnapshot>,
    resolver: Arc<dyn Resolver>,
    audit: Arc<dyn AuditSink>,
    limits: Limits,
    non_tls_ports: HashSet<NonZeroU16>,
}

impl Proxy {
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
            snapshot,
            resolver,
            audit,
            limits,
            non_tls_ports,
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub(crate) fn audit(&self) -> Arc<dyn AuditSink> {
        Arc::clone(&self.audit)
    }

    /// Serves one connection whose caller is already authenticated as `agent`.
    pub async fn serve<C>(&self, mut client: C, agent: &AgentId)
    where
        C: AsyncRead + AsyncWrite + Unpin,
    {
        if let Err(error) = self.run(&mut client, agent).await {
            tracing::debug!(%agent, error = %error, "connection ended with an i/o error");
        }
        let _ = client.shutdown().await;
    }

    async fn run<C>(&self, client: &mut C, agent: &AgentId) -> io::Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin,
    {
        let head = timeout(
            self.limits.head_timeout,
            read_request(client, self.limits.max_head_bytes),
        )
        .await;
        let (request, leftover) = match head {
            Err(_) => {
                tracing::debug!(%agent, "request head timed out");
                return Ok(());
            }
            Ok(Err(RequestError::Closed)) => return Ok(()),
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

        if let Admission::Refuse(fault) = admit(self.snapshot.as_ref(), agent, &destination, SystemTime::now()) {
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
        let targets = match dial_targets(
            self.snapshot.as_ref(),
            agent,
            destination.port(),
            &candidates,
            SystemTime::now(),
        ) {
            Ok(targets) => targets,
            Err(fault) => return self.refuse(client, agent, Some(&destination), None, fault).await,
        };
        let Some((mut upstream, address)) = self.dial(&targets).await else {
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
                    let _ = self
                        .record(
                            agent,
                            Some(&destination),
                            resolved,
                            Decision::Refused(Fault::SniMismatch),
                        )
                        .await;
                    return Ok(());
                }
            }
        }

        upstream.write_all(&pending).await?;
        copy_bidirectional(client, &mut upstream).await?;
        Ok(())
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
    async fn dial(&self, targets: &[SocketAddr]) -> Option<(TcpStream, SocketAddr)> {
        for target in targets {
            let attempt = timeout(self.limits.connect_timeout, TcpStream::connect(target))
                .await
                .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()));
            match attempt {
                Ok(stream) => return Some((stream, *target)),
                Err(error) => tracing::debug!(error = %error, "upstream connect failed"),
            }
        }
        None
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
    ) -> io::Result<()>
    where
        C: AsyncWrite + Unpin,
    {
        if fault != Fault::AuditUnavailable {
            let _ = self
                .record(agent, destination, resolved, Decision::Refused(fault))
                .await;
        }
        client.write_all(&fault.response()).await?;
        client.flush().await
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
