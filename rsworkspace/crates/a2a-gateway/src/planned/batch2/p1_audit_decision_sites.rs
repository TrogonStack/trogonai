//! Phase 1 — Gateway audit decision sites (`rules_fired`, allow/deny, rewrites).
//!
//! **Status today:** gateway ingress is opaque forward-only; no policy engine and no decision
//! audits. Agent-side [`Bridge::dispatch`](a2a_nats::agent::bridge::Bridge::dispatch) emits
//! per-RPC [`AuditEnvelope`](a2a_nats::audit::AuditEnvelope) when a non-noop
//! [`AuditEmitter`](a2a_nats::audit::AuditEmitter) is configured.
//!
//! **Audit partials ([`A2A_PLAN.md`](../../../../A2A_PLAN.md)):** Phase 1 pending — gateway
//! decision-site wiring and full envelope schema (§Audit example: `trace_id`, `rules_fired`,
//! `rewrites`, `stream_consumer`; no `tenant` field per §Decisions). Gateway policy attribution
//! fields remain future once ingress policy sites land.
//!
//! **Target:** at each Tier 1 / SpiceDB / redaction boundary, record which rules fired, the
//! allow/deny outcome, and any rewrite summary. Published ingress audits should populate optional
//! extras using the same field names as
//! [`AuditEnvelopeFields`](a2a_nats::audit::AuditEnvelopeFields) (`rules_fired`, `rewrites`).

use std::fmt;

/// Allow/deny decision recorded at a gateway ingress policy site (maps to §Audit `decision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressOutcome {
    Allow,
    Deny,
}

impl fmt::Display for IngressOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
        }
    }
}

/// Draft payload collected at a policy decision site before enrichment into a full audit envelope.
///
/// `rules_fired` aligns with [`AuditEnvelopeFields::rules_fired`](a2a_nats::audit::AuditEnvelopeFields).
/// `rewrite_note` is a gateway-local summary until full `rewrites` JSON is serialized at publish time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressAuditDraft {
    pub outcome: IngressOutcome,
    pub rules_fired: Vec<String>,
    pub rewrite_note: Option<String>,
}

/// Hook for emitting gateway ingress audit drafts at policy decision sites.
pub trait IngressAuditEmitter: Send + Sync {
    fn emit(&self, draft: &IngressAuditDraft);
}

/// Default stub until JetStream ingress audit wiring lands.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopIngressAuditEmitter;

impl IngressAuditEmitter for NoopIngressAuditEmitter {
    fn emit(&self, _draft: &IngressAuditDraft) {}
}
