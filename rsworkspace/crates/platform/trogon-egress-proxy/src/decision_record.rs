//! Maps what the proxy knows about one decision onto the platform audit record
//! (ADR#0086 Decision 5). Every field is a name or a closed code from the
//! proxy's own vocabulary, so no header value and no tunnel byte has a field to
//! land in.
//!
//! | proxy fact                | record field                                   |
//! |---------------------------|------------------------------------------------|
//! | calling agent             | `actor` (the agent arm)                        |
//! | destination `host:port`   | `gateway_decision.target`                      |
//! | permitted or refused      | `attribution.decision`, enforced, and the subject terminal |
//! | refusal fault             | `attribution.reason_code`, lower snake case    |
//! | policy snapshot kind      | `attribution.policy_source`                    |
//! | never intercepted         | `gateway_decision.tls_mode` = passthrough      |
//! | configured tenant, project| `tenant`, `project`                            |
//!
//! Left absent because the proxy cannot say them honestly: the session (the
//! certificate profile that names it is a later ADR), the rule ids and rule
//! refs (these snapshots have no rule identity), byte counts and latency (a tunnel is opaque and the
//! decision precedes it). The resolved address has no field in the record and
//! stays in the local log line only.
//! Live snapshots bind their configured bucket and incarnation in the policy
//! source and retain the observed producer generation as the projection revision.

use trogon_audit::{
    Actor, AuditName, AuditNameError, DecisionFacts, DecisionOutcome, Enforcement, ReasonCode, Record, RecordScope,
    Stamp, Surface, TlsMode,
};
use trogon_nats::NatsToken;

use crate::audit::{AuditEvent, Decision};
use crate::audit_error::AuditError;
use crate::constants::{DESTINATION_TOO_LONG_TARGET, NO_DESTINATION_TARGET};
use crate::destination::Destination;
use crate::policy_source::PolicySource;

pub(crate) fn decision_record(
    event: &AuditEvent<'_>,
    tenant: &AuditName,
    project: &NatsToken,
    stamp: Stamp,
) -> Result<Record, AuditError> {
    let provenance_matches = match (event.snapshot_source, event.policy_revision) {
        (PolicySource::StaticFile { .. }, None) => true,
        (PolicySource::NatsKv { incarnation, .. }, Some(revision)) => *incarnation == revision.incarnation,
        (PolicySource::NatsKv { .. }, None) => matches!(event.decision, Decision::Refused(_)),
        _ => false,
    };
    if !provenance_matches {
        return Err(AuditError::PolicyEvidence);
    }
    let actor = Actor::Agent {
        agent_id: AuditName::new(event.agent.as_str())?,
    };
    let scope = RecordScope::new(tenant.clone(), project.clone(), actor);
    let mut facts = DecisionFacts::new(Surface::Egress, target(event.destination)?, outcome(event.decision));
    facts.reason = reason(event.decision)?;
    facts.policy_source = Some(AuditName::new(event.snapshot_source.audit_name())?);
    facts.projection_revision = event.policy_revision.map(|revision| revision.generation.get());
    facts.tls_mode = Some(TlsMode::Passthrough);
    match event.workload_peer {
        Some(peer) => {
            Record::authenticated_workload_decision(scope, facts, peer.clone(), stamp).map_err(AuditError::from)
        }
        None => Ok(Record::decision(scope, facts, stamp)),
    }
}

fn outcome(decision: Decision) -> DecisionOutcome {
    match decision {
        Decision::Permitted => DecisionOutcome::Allow(Enforcement::Enforce),
        Decision::Refused(_) => DecisionOutcome::Deny,
    }
}

fn reason(decision: Decision) -> Result<Option<ReasonCode>, AuditError> {
    decision
        .fault()
        .map(|fault| ReasonCode::new(fault.reason_code()))
        .transpose()
        .map_err(AuditError::from)
}

/// The destination as `host:port`, or a name that says there was none.
fn target(destination: Option<&Destination>) -> Result<AuditName, AuditNameError> {
    let Some(destination) = destination else {
        return AuditName::new(NO_DESTINATION_TARGET);
    };
    AuditName::new(destination.to_string()).or_else(|_| AuditName::new(DESTINATION_TOO_LONG_TARGET))
}

#[cfg(test)]
mod tests;
