//! One structured event per decision (ADR#0085 Decision 5, ADR#0084).
//!
//! An event names who asked, for what, where it resolved, what was decided and
//! on which policy snapshot. It carries no header value and no byte of the
//! tunnel, so the audit trail cannot become a second copy of the traffic.

use std::net::IpAddr;

use crate::agent_id::AgentId;
use crate::destination::Destination;
use crate::fault::Fault;
use crate::policy_source::PolicySource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Permitted,
    Refused(Fault),
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permitted => "permitted",
            Self::Refused(_) => "refused",
        }
    }

    pub fn fault(self) -> Option<Fault> {
        match self {
            Self::Permitted => None,
            Self::Refused(fault) => Some(fault),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent<'a> {
    pub agent: &'a AgentId,
    /// Absent when the request never produced a valid destination.
    pub destination: Option<&'a Destination>,
    pub resolved_address: Option<IpAddr>,
    pub decision: Decision,
    pub snapshot_source: &'a PolicySource,
}

/// Where decisions are recorded. Implementations must not block: the call sits
/// on the connection's path.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: &AuditEvent<'_>);
}

/// Emits each decision as a `tracing` event on the `egress_audit` target.
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingAudit;

impl AuditSink for TracingAudit {
    fn record(&self, event: &AuditEvent<'_>) {
        let destination = event.destination.map(ToString::to_string);
        let resolved = event.resolved_address.map(|address| address.to_string());
        tracing::info!(
            target: "egress_audit",
            agent = %event.agent,
            destination = destination.as_deref().unwrap_or("-"),
            resolved_address = resolved.as_deref().unwrap_or("-"),
            decision = event.decision.as_str(),
            fault = event.decision.fault().map_or("-", Fault::code),
            snapshot_source = %event.snapshot_source,
            "egress decision"
        );
    }
}

#[cfg(test)]
mod tests;
