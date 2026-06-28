//! A2A ingress audit envelope.
//!
//! Subject grammar: `{prefix}.a2a.audit.{outcome}.ingress.{skill}`
//! where `outcome` is rendered by [`IngressAuditOutcome::as_str`] and
//! the skill segment is the agent-card skill ID with subject-unsafe
//! characters replaced by `_` so each segment remains a single NATS
//! token.

use a2a_redaction::SkillId;
use serde::{Deserialize, Deserializer, Serialize};
use trogon_nats::{NatsToken, SubjectTokenViolation};

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
///
/// `Deserialize` is routed through [`Self::new`] (mirroring the
/// `SkillId` pattern) so wire-borne values can't bypass the
/// constructor's validation via a transparent serde derive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

impl<'de> Deserialize<'de> for AuditRequestId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// Non-empty tenant identifier carried alongside the audit envelope
/// so multi-tenant consumers can route per-tenant without rebuilding
/// a tenant from the agent id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

impl<'de> Deserialize<'de> for AuditTenant {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// Non-empty agent identifier scoped to the audit envelope. Mirrors
/// the `A2aAgentId` constraint of being a single NATS-safe token,
/// but kept as a local newtype so the envelope's serde wire shape
/// stays decoupled from upstream NATS-routing changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

impl<'de> Deserialize<'de> for AuditAgent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// W3C traceparent. Validation is intentionally light (non-empty
/// only) so the upstream `tracing` machinery — which produces these
/// strings via its own validated context — remains the source of
/// truth for shape; we just refuse to persist an empty marker that
/// would alias every untraced request to the same correlation key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

impl<'de> Deserialize<'de> for AuditTraceparent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(raw).map_err(serde::de::Error::custom)
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

#[derive(Clone, Debug, thiserror::Error)]
pub enum IngressAuditSubjectError {
    /// The escaped skill segment failed `NatsToken` validation —
    /// usually because the skill id contained non-ASCII or its
    /// escape expansion overran the per-token length cap. Fail
    /// closed here rather than emitting a subject the publisher
    /// would later reject.
    #[error("encoded skill segment is not a valid NATS token: {0}")]
    InvalidSkillSegment(#[source] SubjectTokenViolation),
}

/// Build the audit publish subject for an outcome + skill.
///
/// Returns `Err(IngressAuditSubjectError::InvalidSkillSegment)`
/// when the escaped skill segment fails `NatsToken` validation
/// (non-ASCII characters, or the escape expansion overran the
/// per-token length cap). Failing closed prevents the gateway
/// from publishing on a subject the NATS server would later
/// reject as malformed.
pub fn ingress_audit_subject(
    prefix: &str,
    outcome: IngressAuditOutcome,
    skill: &SkillId,
) -> Result<String, IngressAuditSubjectError> {
    let token = skill_subject_token(skill.as_str());
    // Validate the produced segment against the same `NatsToken`
    // grammar used by the rest of the stack so we can't emit a
    // subject the publisher would reject.
    NatsToken::new(&token).map_err(IngressAuditSubjectError::InvalidSkillSegment)?;
    Ok(format!("{prefix}.a2a.audit.{}.ingress.{token}", outcome.as_str()))
}

/// Reversible escape for NATS-unsafe characters in a skill id.
///
/// Maps the small set of subject-breaking chars to `_<tag>`
/// sequences and escapes existing `_` to `__` so distinct skill ids
/// can't collide on the same audit subject. Without the underscore
/// double-up, `docs.search` and `docs_search` would both publish to
/// `…ingress.docs_search`, silently aliasing two different skills.
///
/// Non-ASCII characters are escaped to `_u<hex>` so the produced
/// token stays inside the ASCII-only `NatsToken` grammar. Length is
/// not bounded here; callers go through [`ingress_audit_subject`]
/// which validates the result against [`NatsToken`].
///
/// `SkillId::new` already rejects ASCII control characters (`\t`,
/// `\n`, …) and the path separators `/`, `\\`, `\0`.
fn skill_subject_token(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '_' => out.push_str("__"),
            '.' => out.push_str("_d"),
            '*' => out.push_str("_s"),
            '>' => out.push_str("_g"),
            c if c.is_whitespace() => out.push_str("_w"),
            c if !c.is_ascii() => {
                // Pad the hex with zeros so different code points
                // can't share a prefix that aliases on truncation.
                out.push_str(&format!("_u{:06x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
