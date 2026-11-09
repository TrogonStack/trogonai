//! One event per decision (ADR#0085 Decision 5, ADR#0084).
//!
//! An event names who asked, for what, where it resolved, what was decided and
//! on which policy snapshot. It carries no header value and no byte of the
//! tunnel, so the audit trail cannot become a second copy of the traffic.
//!
//! Recording is awaited and returns its outcome, because a permit is only as
//! good as its record: [`crate::connection::Proxy`] refuses the tunnel with
//! `AUDIT_UNAVAILABLE` when the record of a permit cannot be stored. What a
//! failed write means for a refusal is the writer's business, not the proxy's.

use std::net::IpAddr;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::agent_id::AgentId;
use crate::audit_error::AuditError;
use crate::destination::Destination;
use crate::fault::Fault;
use crate::policy_revision::PolicyRevision;
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
    pub policy_revision: Option<PolicyRevision>,
}

/// Where decisions are recorded.
#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Records one decision. For [`Decision::Permitted`], `Err` means the
    /// tunnel must not open. For a refusal the answer is sent regardless, so
    /// the caller ignores `Err`.
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError>;

    /// Background work the sink needs for as long as the server runs, such as
    /// retrying buffered records. Returns once `shutdown` is cancelled and the
    /// sink has flushed what it can. Most sinks have none.
    async fn run(&self, _shutdown: CancellationToken) {}
}

#[cfg(test)]
mod tests;
