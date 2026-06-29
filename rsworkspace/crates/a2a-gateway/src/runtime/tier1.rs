//! Tier-1 dispatch-side helpers.
//!
//! Pure functions that translate raw ingress fields (method dots,
//! caller slug, agent id, NATS subject) into the typed Tier-1
//! context the policy gate consumes. Keeping the wire-shape
//! parsing out of the dispatch path lets the orchestrator stay
//! flat and the Tier-1 evaluator stay focused on policy.

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::A2aMethod;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::audit::envelope::AuditEnvelopeFields;

use crate::policy::spicedb_tier1::tier1_principal_from_caller;
use crate::policy::tier1_declarative::Tier1DeclarativeContext;

/// Build the Tier-1 declarative evaluation context for an ingress
/// request. Returns `None` when the method dots don't resolve to a
/// typed `A2aMethod` -- the gateway routes those to the
/// invalid-method error path rather than running the policy at
/// all.
///
/// The caller slug is normalized so a value of either form
/// (`user/alice` or `alice`) lands at the same `SpiceDbSubject`,
/// and an absent caller resolves to the anonymous placeholder so
/// the declarative bundle can deny anonymous callers explicitly
/// rather than skipping the evaluation entirely.
pub fn tier1_declarative_context_from_ingress(
    method_dots: &str,
    agent_id: &A2aAgentId,
    caller_slug: Option<&str>,
    account: &str,
    nats_subject: &str,
) -> Option<Tier1DeclarativeContext> {
    let method = A2aMethod::from_dotted_suffix(method_dots)?;
    let raw_slug = caller_slug.unwrap_or(ANONYMOUS_CALLER_SLUG);
    let slug = raw_slug.split_once('/').map(|(_, id)| id).unwrap_or(raw_slug);
    let principal = tier1_principal_from_caller(slug, account);
    // The principal's `spicedb_subject()` returns
    // `a2a_identity_types::SpiceDbSubject`, but the declarative
    // evaluator on main was built against
    // `a2a_auth_callout::SpiceDbSubject`. Bridge through the bare
    // string -- the two types are semantically the same SpiceDB
    // subject reference; unifying them is a separate refactor
    // outside this slice.
    let caller_subject = principal
        .spicedb_subject()
        .map(|subject| SpiceDbSubject::new(subject.as_str()));
    Some(Tier1DeclarativeContext::new(
        method,
        agent_id.clone(),
        caller_subject,
        nats_subject,
    ))
}

/// Sentinel slug used when an ingress request carries no caller
/// identity. Distinct from any real caller value so a deny-anonymous
/// declarative rule can match on it precisely. Kept as a single
/// constant so the value matches between the context builder and
/// the bundle authors who write rules against it.
pub const ANONYMOUS_CALLER_SLUG: &str = "_";

/// Stamp the caller-correlation fields onto an audit envelope
/// extras block. Lives on the dispatch side because the denial
/// flow has the caller pair handy but the audit envelope factory
/// doesn't know about ingress identity.
pub fn enrich_audit_caller(
    mut fields: AuditEnvelopeFields,
    caller_id: &str,
    caller_source: &Option<String>,
) -> AuditEnvelopeFields {
    fields.caller_id = Some(caller_id.to_owned());
    fields.caller_source = caller_source.clone();
    fields
}

#[cfg(test)]
mod tests;
