//! Maps what the gateway knows about one decision onto the platform audit record
//! (ADR#0086 Decision 1). The legacy `AuditEnvelope` stays the source type of the
//! best-effort emitter and this is the adapter beside it.
//!
//! | gateway fact                       | record field                                      |
//! |------------------------------------|---------------------------------------------------|
//! | caller principal (agent)           | `actor`, the agent arm                            |
//! | caller principal (person)          | `actor`, a human session issued by the tenant     |
//! | caller principal (service)         | `actor`, unattributed (no service arm exists)     |
//! | method and agent                   | `gateway_decision.target`, `message/send@agent`   |
//! | deny or allow, always enforced     | `attribution.decision` and the subject terminal   |
//! | refusing layer                     | `reason_code` and one layer verdict               |
//! | `rules_fired`                      | rule ids                                          |
//! | SpiceDB consistency token          | the Tier-1 layer verdict                          |
//! | trace id, JSON-RPC id              | `trace_id`, `request_id`                          |
//! | request size, latency              | `request_bytes`, `latency`                        |
//!
//! Left out on purpose: the params fingerprint, every message of the legacy
//! envelope (they can echo content), the rewrites and the stream consumer (routing
//! detail), and the response size (the reply is the agent's, after the decision).

use trogon_audit::{
    Actor, AuditName, ConsistencyToken, DecisionFacts, DecisionOutcome, Enforcement, LayerFacts, ReasonCode, Record,
    RecordScope, Stamp, Surface, TraceId,
};
use trogon_identity_types::{PrincipalKind, SubjectPrincipal};
use trogon_nats::NatsToken;

use super::audit_error::AuditError;
use super::audit_names::name_of;
use super::decision_event::{AllowedLayers, DecisionEvent, Verdict};

const TIER1_LAYER: &str = "tier1_spicedb";
const TIER3_LAYER: &str = "tier3";

pub(crate) fn decision_record(
    event: &DecisionEvent<'_>,
    tenant: &AuditName,
    project: &NatsToken,
    stamp: Stamp,
) -> Result<Record, AuditError> {
    let mut scope = RecordScope::new(tenant.clone(), project.clone(), actor(event.caller, tenant)?);
    if let Some(trace_id) = trace_id(event.trace_id) {
        scope = scope.with_trace_id(trace_id);
    }
    if let Some(request_id) = event.request_id.and_then(|id| AuditName::new(id).ok()) {
        scope = scope.with_request_id(request_id);
    }

    let target = name_of(&format!("{}@{}", event.method, event.agent_id.as_str()))?;
    let mut facts = DecisionFacts::new(Surface::A2aIngress, target, outcome(&event.verdict));
    facts.read_only = event.read_only;
    facts.rule_ids = event
        .rules_fired
        .iter()
        .map(|rule| name_of(rule))
        .collect::<Result<_, _>>()?;
    facts.request_bytes = Some(u64::try_from(event.request_bytes).unwrap_or(u64::MAX));
    facts.latency = Some(event.latency);
    match &event.verdict {
        Verdict::Allowed(layers) => facts.layers = allowed_layers(*layers)?,
        Verdict::Denied(reason) => {
            let code = ReasonCode::new(reason.code())?;
            facts.reason = Some(code);
            let mut layer = LayerFacts::new(AuditName::new(reason.layer())?, DecisionOutcome::Deny);
            layer.reason = Some(code);
            facts.layers = vec![layer];
        }
    }
    Ok(Record::decision(scope, facts, stamp))
}

fn outcome(verdict: &Verdict<'_>) -> DecisionOutcome {
    match verdict {
        Verdict::Allowed(_) => DecisionOutcome::Allow(Enforcement::Enforce),
        Verdict::Denied(_) => DecisionOutcome::Deny,
    }
}

/// An agent is an agent and a person is a session the tenant's callout issued.
/// A service has no arm, and the record cannot call it an agent or a person
/// without claiming something that is not true, so it is unattributed here; the
/// proto needs a service arm before such a caller can be named.
fn actor(caller: &SubjectPrincipal, tenant: &AuditName) -> Result<Actor, AuditError> {
    let id = name_of(caller.id().as_str())?;
    Ok(match caller.kind() {
        PrincipalKind::Agent => Actor::Agent { agent_id: id },
        PrincipalKind::Person => Actor::HumanSession {
            subject: id,
            issuer: tenant.clone(),
        },
        PrincipalKind::Service => Actor::Unattributed,
    })
}

fn allowed_layers(layers: AllowedLayers<'_>) -> Result<Vec<LayerFacts>, AuditError> {
    let mut verdicts = Vec::with_capacity(2);
    if layers.tier1_spicedb {
        let mut tier1 = LayerFacts::new(
            AuditName::new(TIER1_LAYER)?,
            DecisionOutcome::Allow(Enforcement::Enforce),
        );
        // A token that is not one a record may carry is left off: the layer still
        // says it allowed, and the token is a convenience for reconstruction.
        tier1.consistency_token = layers
            .tier1_consistency_token
            .and_then(|token| ConsistencyToken::new(token).ok());
        verdicts.push(tier1);
    }
    if layers.tier3 {
        verdicts.push(LayerFacts::new(
            AuditName::new(TIER3_LAYER)?,
            DecisionOutcome::Allow(Enforcement::Enforce),
        ));
    }
    Ok(verdicts)
}

/// The gateway's trace id is a UUID; a W3C trace id is the same 128 bits without the hyphens.
fn trace_id(trace_id: &str) -> Option<TraceId> {
    TraceId::new(trace_id.replace('-', "")).ok()
}

#[cfg(test)]
mod tests;
