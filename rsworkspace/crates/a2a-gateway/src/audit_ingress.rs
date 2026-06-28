//! A2A ingress audit envelope.
//!
//! Subject grammar: `{prefix}.a2a.audit.{outcome}.ingress.{skill}`
//! where `outcome` is rendered by [`IngressAuditOutcome::as_str`] and
//! the skill segment is the agent-card skill ID with subject-unsafe
//! characters replaced by `_` so each segment remains a single NATS
//! token.

use a2a_redaction::SkillId;
use serde::{Deserialize, Serialize};

use crate::policy::per_skill::PerSkillDecision;

/// Typed audit outcome — modelled as an enum so a renamed string in
/// the subject grammar would break callsites at compile time rather
/// than producing audit subjects pointed at a topic nobody reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngressAuditOutcome {
    Allow,
    Deny,
}

impl IngressAuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum A2aIngressAuditBuildError {
    #[error("ingress audit request_id must not be empty")]
    EmptyRequestId,
    #[error("ingress audit tenant must not be empty")]
    EmptyTenant,
    #[error("ingress audit agent must not be empty")]
    EmptyAgent,
    #[error("ingress audit traceparent must not be empty when present")]
    EmptyTraceparent,
}

/// Non-empty audit request id. Carried into downstream audit
/// subscribers as the correlation key — failing closed at the
/// boundary avoids persisting an empty key that breaks join queries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditRequestId(String);

impl AuditRequestId {
    pub fn new(value: impl Into<String>) -> Result<Self, A2aIngressAuditBuildError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(A2aIngressAuditBuildError::EmptyRequestId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Non-empty tenant identifier carried alongside the audit envelope
/// so multi-tenant consumers can route per-tenant without rebuilding
/// a tenant from the agent id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditTenant(String);

impl AuditTenant {
    pub fn new(value: impl Into<String>) -> Result<Self, A2aIngressAuditBuildError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(A2aIngressAuditBuildError::EmptyTenant);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Non-empty agent identifier scoped to the audit envelope. Mirrors
/// the `A2aAgentId` constraint of being a single NATS-safe token,
/// but kept as a local newtype so the envelope's serde wire shape
/// stays decoupled from upstream NATS-routing changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditAgent(String);

impl AuditAgent {
    pub fn new(value: impl Into<String>) -> Result<Self, A2aIngressAuditBuildError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(A2aIngressAuditBuildError::EmptyAgent);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// W3C traceparent. Validation is intentionally light (non-empty
/// only) so the upstream `tracing` machinery — which produces these
/// strings via its own validated context — remains the source of
/// truth for shape; we just refuse to persist an empty marker that
/// would alias every untraced request to the same correlation key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditTraceparent(String);

impl AuditTraceparent {
    pub fn new(value: impl Into<String>) -> Result<Self, A2aIngressAuditBuildError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(A2aIngressAuditBuildError::EmptyTraceparent);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Ingress audit envelope. Every field is a validated domain value
/// object so an envelope that reaches storage can't carry
/// boundary-untyped strings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct A2aIngressAudit {
    pub request_id: AuditRequestId,
    pub tenant: AuditTenant,
    pub agent: AuditAgent,
    pub skill: SkillId,
    pub outcome: IngressAuditOutcome,
    pub reason: Option<String>,
    pub shadow: bool,
    pub traceparent: Option<AuditTraceparent>,
}

impl A2aIngressAudit {
    /// Build the audit envelope from already-validated value objects
    /// plus a per-skill decision. Boundary validation lives in the
    /// individual value-object constructors so this builder accepts
    /// only typed inputs and never freezes raw boundary strings.
    pub fn from_decision(
        request_id: AuditRequestId,
        tenant: AuditTenant,
        agent: AuditAgent,
        skill: SkillId,
        decision: &PerSkillDecision,
        traceparent: Option<AuditTraceparent>,
    ) -> Self {
        let (outcome, reason, shadow) = classify_decision(decision, false);
        Self {
            request_id,
            tenant,
            agent,
            skill,
            outcome,
            reason,
            shadow,
            traceparent,
        }
    }
}

/// Recursively unwrap nested shadow wrappers so the audit envelope
/// always carries the *would-be* terminal outcome. Without this an
/// accidentally double-wrapped Shadow would surface as a synthetic
/// "unknown" tag the audit consumer doesn't know how to route.
fn classify_decision(decision: &PerSkillDecision, shadow_so_far: bool) -> (IngressAuditOutcome, Option<String>, bool) {
    match decision {
        PerSkillDecision::Allow { .. } => (IngressAuditOutcome::Allow, None, shadow_so_far),
        PerSkillDecision::Deny { reason, .. } => (IngressAuditOutcome::Deny, Some(reason.clone()), shadow_so_far),
        PerSkillDecision::Shadow { would_be } => classify_decision(would_be, true),
    }
}

/// Build the audit publish subject for an outcome + skill.
#[must_use]
pub fn ingress_audit_subject(prefix: &str, outcome: IngressAuditOutcome, skill: &SkillId) -> String {
    let token = skill_subject_token(skill.as_str());
    format!("{prefix}.a2a.audit.{}.ingress.{token}", outcome.as_str())
}

fn skill_subject_token(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            // `.` splits into two NATS tokens; `*` and `>` are NATS
            // wildcard markers and would let a pathological skill id
            // subscribe to / publish on subjects it shouldn't. Plain
            // ASCII space also breaks the single-token-per-segment
            // rule.
            //
            // `SkillId::new` already rejects ASCII control characters
            // (including `\t`, `\n`, `\r`) and the path separators
            // `/` and `\\` at construction, so this helper never sees
            // those — the `is_whitespace()` branch is defense-in-depth
            // for any future SkillId loosening rather than a real
            // path today.
            if c.is_whitespace() || matches!(c, '.' | '*' | '>') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
