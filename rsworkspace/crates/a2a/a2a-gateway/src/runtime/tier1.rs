//! Tier-1 dispatch-side helpers.
//!
//! Pure functions that translate raw ingress fields (method dots, the
//! caller the ingress subject names, agent id, NATS subject) into the
//! typed Tier-1 context the policy gate consumes. Keeping the wire-shape
//! parsing out of the dispatch path lets the orchestrator stay flat and
//! the Tier-1 evaluator stay focused on policy.

use a2a_nats::A2aMethod;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::audit::envelope::AuditEnvelopeFields;
use trogon_identity_types::SubjectPrincipal;

use crate::policy::CallerSpiceDbSubject;
use crate::policy::tier1_declarative::Tier1DeclarativeContext;

/// Build the Tier-1 declarative evaluation context for an ingress
/// request. Returns `None` when the method dots don't resolve to a
/// typed `A2aMethod` -- the gateway routes those to the
/// invalid-method error path rather than running the policy at
/// all.
pub fn tier1_declarative_context_from_ingress(
    method_dots: &str,
    agent_id: &A2aAgentId,
    caller: &CallerSpiceDbSubject,
    nats_subject: &str,
) -> Option<Tier1DeclarativeContext> {
    let method = A2aMethod::from_dotted_suffix(method_dots)?;
    Some(Tier1DeclarativeContext::new(
        method,
        agent_id.clone(),
        caller.to_spicedb_subject(),
        nats_subject,
    ))
}

/// Stamp the caller the ingress subject named onto an audit envelope
/// extras block. Lives on the dispatch side because the denial flow has
/// the caller handy but the audit envelope factory doesn't know about
/// ingress identity.
pub fn enrich_audit_caller(mut fields: AuditEnvelopeFields, caller: &SubjectPrincipal) -> AuditEnvelopeFields {
    fields.caller_id = Some(caller.to_string());
    fields
}

#[cfg(test)]
mod tests;
