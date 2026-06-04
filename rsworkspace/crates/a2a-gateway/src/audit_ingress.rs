//! A2A ingress audit envelope.
//!
//! Subject grammar: `{prefix}.a2a.audit.{outcome}.ingress.{skill}`
//! where `outcome` ∈ `{"allow", "deny"}`. The skill segment is the
//! agent-card skill ID, with `.` replaced by `_` to keep the
//! subject single-token-per-segment per NATS rules.

use a2a_redaction::skill_id::SkillId;
use serde::{Deserialize, Serialize};

use crate::policy::per_skill::PerSkillDecision;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct A2aIngressAudit {
    pub request_id: String,
    pub tenant: String,
    pub agent: String,
    pub skill: SkillId,
    pub outcome: String,
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
        let (outcome, reason, shadow) = match decision {
            PerSkillDecision::Allow { .. } => ("allow".to_string(), None, false),
            PerSkillDecision::Deny { reason, .. } => ("deny".to_string(), Some(reason.clone()), false),
            PerSkillDecision::Shadow { would_be } => match would_be.as_ref() {
                PerSkillDecision::Allow { .. } => ("allow".to_string(), None, true),
                PerSkillDecision::Deny { reason, .. } => ("deny".to_string(), Some(reason.clone()), true),
                PerSkillDecision::Shadow { .. } => ("unknown".to_string(), None, true),
            },
        };
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

/// Build the audit publish subject for an outcome + skill.
#[must_use]
pub fn ingress_audit_subject(prefix: &str, outcome: &str, skill: &SkillId) -> String {
    let token = skill_subject_token(skill.as_str());
    format!("{prefix}.a2a.audit.{outcome}.ingress.{token}")
}

fn skill_subject_token(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '.' | ' ' | '*' | '>' => '_',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::per_skill::ResolvedRule;

    #[test]
    fn subject_segments_skill_with_dots() {
        let subject = ingress_audit_subject("a2a", "allow", &SkillId::new("docs.search"));
        assert_eq!(subject, "a2a.a2a.audit.allow.ingress.docs_search");
    }

    #[test]
    fn audit_envelope_from_allow_decision() {
        let dec = PerSkillDecision::Allow { contributor: None };
        let audit = A2aIngressAudit::from_decision(
            "req-1",
            "acme",
            "planner",
            SkillId::new("search"),
            &dec,
            Some("00-aaa-bbb-01".into()),
        );
        assert_eq!(audit.outcome, "allow");
        assert!(!audit.shadow);
    }

    #[test]
    fn audit_envelope_marks_shadow() {
        let dec = PerSkillDecision::Shadow {
            would_be: Box::new(PerSkillDecision::Deny {
                reason: "forbidden".into(),
                contributor: None,
            }),
        };
        let audit = A2aIngressAudit::from_decision(
            "req-2",
            "acme",
            "planner",
            SkillId::new("search"),
            &dec,
            None,
        );
        assert_eq!(audit.outcome, "deny");
        assert_eq!(audit.reason.as_deref(), Some("forbidden"));
        assert!(audit.shadow);
    }

    #[test]
    fn resolved_rule_round_trip_shape() {
        // Compile-only check that ResolvedRule is reachable as part of the public surface.
        let _ = std::mem::size_of::<ResolvedRule>();
    }
}
