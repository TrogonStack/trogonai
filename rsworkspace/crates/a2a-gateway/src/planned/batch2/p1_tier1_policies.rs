//! Phase 1 — Tier 1 declarative policies on the gateway request path.
//!
//! **Status today:** gateway ingress is an opaque forward — no Tier 1 evaluation runs before the
//! agent publish. This module is compile-only scaffolding for the future ingress decision site.
//!
//! **Target:** evaluate signed bundle rules on `{prefix}.gateway.{agent_id}.{method…}` **before**
//! forwarding to `{prefix}.agent.{agent_id}.{method…}`. Outcomes are allow, deny (JSON-RPC error
//! code), or subject-tail rewrite. Fired rule ids feed gateway audit `rules_fired` attribution.
//!
//! Plan: [`A2A_PLAN.md`](../../../../A2A_PLAN.md) §Policy Engine (Tier 1 declarative / SpiceDB tuples).
//! Roadmap: [`A2A_GATEWAY_ROADMAP.md`](../../../../docs/A2A_GATEWAY_ROADMAP.md) §Coordination with SpiceDB Tier 1.
//!
//! Tier 2 CEL and Tier 3 WASM redaction live in [`super::p2_wasmtime_substrate`] and run after Tier 1
//! allow on the same ingress path.

use std::fmt;

/// Signed policy bundle identity hot-swapped into the gateway (first-party pack: `a2a-pack`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IngressPolicyBundleId(pub String);

impl IngressPolicyBundleId {
    /// Placeholder id for the first-party A2A bundle until signing and distribution land.
    pub fn first_party_a2a_pack() -> Self {
        Self("a2a-pack".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IngressPolicyBundleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tier 1 ingress outcome — declarative allow/deny/rewrite before opaque forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Proceed with forward (optionally after applying a subject rewrite).
    Allow,
    /// Short-circuit with a JSON-RPC error code to the caller reply inbox.
    Deny { code: i32 },
    /// Replace the dotted method tail on the agent subject before forward.
    RewriteSubject { tail: String },
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    pub fn deny_code(&self) -> Option<i32> {
        match self {
            Self::Deny { code } => Some(*code),
            _ => None,
        }
    }

    pub fn rewrite_tail(&self) -> Option<&str> {
        match self {
            Self::RewriteSubject { tail } => Some(tail.as_str()),
            _ => None,
        }
    }
}

/// Rule identifiers emitted to gateway audit `rules_fired` (see plan §Audit).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesFired(pub Vec<&'static str>);

impl RulesFired {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn record(&mut self, rule: &'static str) {
        self.0.push(rule);
    }

    pub fn as_slice(&self) -> &[&'static str] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<&'static str> {
        self.0
    }
}

impl fmt::Display for RulesFired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join(","))
    }
}

/// Declarative Tier 1 policy surface — bundle tables, allowlists, catalog shaping hooks.
///
/// `method_suffix` uses the dotted tail from gateway ingress (`"message.send"`, `"tasks.get"`, …).
pub trait Tier1IngressPolicy: Send + Sync {
    fn evaluate(&self, method_suffix: &str) -> Decision;
}

/// Combined Tier 1 outcome for the ingress decision site (policy + audit attribution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1IngressOutcome {
    pub decision: Decision,
    pub rules_fired: RulesFired,
}

impl Tier1IngressOutcome {
    pub fn allow(rules_fired: RulesFired) -> Self {
        Self {
            decision: Decision::Allow,
            rules_fired,
        }
    }

    pub fn deny(code: i32, rules_fired: RulesFired) -> Self {
        Self {
            decision: Decision::Deny { code },
            rules_fired,
        }
    }
}

/// Evaluate Tier 1 **before** opaque forward; records which bundle revision was active.
pub fn evaluate_tier1_before_forward<P: Tier1IngressPolicy>(
    policy: &P,
    bundle_id: &IngressPolicyBundleId,
    method_suffix: &str,
) -> Tier1IngressOutcome {
    let _ = bundle_id;
    let decision = policy.evaluate(method_suffix);
    let mut rules_fired = RulesFired::new();
    match &decision {
        Decision::Allow => rules_fired.record("a2a-tier1-allow"),
        Decision::Deny { .. } => rules_fired.record("a2a-tier1-deny"),
        Decision::RewriteSubject { .. } => {
            rules_fired.record("a2a-tier1-rewrite-subject");
        }
    }
    Tier1IngressOutcome {
        decision,
        rules_fired,
    }
}

/// Compile-only stub: allow every known method suffix (today's effective behavior).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubAllowAllTier1Policy;

impl Tier1IngressPolicy for StubAllowAllTier1Policy {
    fn evaluate(&self, _method_suffix: &str) -> Decision {
        Decision::Allow
    }
}

/// Minimal declarative allowlist — denies unknown method tails with JSON-RPC invalid request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodAllowlistTier1Policy {
    pub allowed_suffixes: Vec<String>,
}

impl Tier1IngressPolicy for MethodAllowlistTier1Policy {
    fn evaluate(&self, method_suffix: &str) -> Decision {
        if self
            .allowed_suffixes
            .iter()
            .any(|suffix| suffix == method_suffix)
        {
            Decision::Allow
        } else {
            Decision::Deny { code: -32600 }
        }
    }
}
