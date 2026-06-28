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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct A2aIngressAudit {
    pub request_id: String,
    pub tenant: String,
    pub agent: String,
    pub skill: SkillId,
    pub outcome: IngressAuditOutcome,
    pub reason: Option<String>,
    pub shadow: bool,
    pub traceparent: Option<String>,
}

impl A2aIngressAudit {
    pub fn from_decision(
        request_id: impl Into<String>,
        tenant: impl Into<String>,
        agent: impl Into<String>,
        skill: SkillId,
        decision: &PerSkillDecision,
        traceparent: Option<String>,
    ) -> Self {
        let (outcome, reason, shadow) = classify_decision(decision, false);
        Self {
            request_id: request_id.into(),
            tenant: tenant.into(),
            agent: agent.into(),
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
        .map(|c| match c {
            // `.` and ` ` would split into two NATS tokens; `*` and
            // `>` are NATS wildcard markers and would let a
            // pathological skill id subscribe to/publish on subjects
            // it shouldn't.
            '.' | ' ' | '*' | '>' => '_',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests;
