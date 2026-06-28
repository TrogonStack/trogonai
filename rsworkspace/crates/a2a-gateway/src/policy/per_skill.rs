//! Per-skill policy bundles.
//!
//! Mirrors `trogon-mcp-gateway`'s hierarchical model but specialized
//! to A2A skills: `(tenant, agent, skill)` is the smallest scope.
//! Resolution order is most-specific-wins; if no rule matches AND
//! `default_deny_when_no_bundles=true`, the request is denied.
//!
//! Shadow mode (`enforce=false`) returns the would-be decision via
//! [`PerSkillDecision::Shadow`] so callers can emit ingress audit
//! without blocking traffic.

use std::collections::HashMap;

use a2a_redaction::SkillId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillEffect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRule {
    pub effect: SkillEffect,
    pub reason: String,
}

impl SkillRule {
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            effect: SkillEffect::Allow,
            reason: reason.into(),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            effect: SkillEffect::Deny,
            reason: reason.into(),
        }
    }
}

/// Scope at which a rule is registered. Determines which lookup
/// it answers and how specific it is for tie-breaking.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SkillScope {
    Skill {
        tenant: String,
        agent: String,
        skill: SkillId,
    },
    Agent {
        tenant: String,
        agent: String,
    },
    Tenant {
        tenant: String,
    },
}

impl SkillScope {
    fn specificity(&self) -> u8 {
        match self {
            Self::Skill { .. } => 3,
            Self::Agent { .. } => 2,
            Self::Tenant { .. } => 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PerSkillRequest {
    pub tenant: String,
    pub agent: String,
    pub skill: SkillId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRule {
    pub scope: SkillScope,
    pub rule: SkillRule,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PerSkillDecision {
    Allow {
        contributor: Option<ResolvedRule>,
    },
    Deny {
        reason: String,
        contributor: Option<ResolvedRule>,
    },
    Shadow {
        would_be: Box<PerSkillDecision>,
    },
}

impl PerSkillDecision {
    /// Outcome label for the ingress audit subject. `Shadow`
    /// unwraps to the inner decision so the audit subject reflects
    /// what *would* have happened.
    pub fn outcome(&self) -> &'static str {
        match self {
            Self::Allow { .. } => "allow",
            Self::Deny { .. } => "deny",
            Self::Shadow { would_be } => would_be.outcome(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PerSkillPolicyConfig {
    /// When `false`, the engine returns [`PerSkillDecision::Shadow`]
    /// wrapping the would-be decision instead of enforcing.
    pub enforce: bool,
    /// When `true` and no rule (at any scope) matches, deny.
    /// When `false`, an unmatched request is allowed.
    pub default_deny_when_no_bundles: bool,
}

impl Default for PerSkillPolicyConfig {
    fn default() -> Self {
        Self {
            enforce: true,
            default_deny_when_no_bundles: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct PerSkillPolicy {
    rules: HashMap<SkillScope, SkillRule>,
    config: PerSkillPolicyConfig,
}

impl PerSkillPolicy {
    pub fn new(config: PerSkillPolicyConfig) -> Self {
        Self {
            rules: HashMap::new(),
            config,
        }
    }

    pub fn upsert(&mut self, scope: SkillScope, rule: SkillRule) {
        self.rules.insert(scope, rule);
    }

    pub fn evaluate(&self, req: &PerSkillRequest) -> PerSkillDecision {
        let decision = self.resolve(req);
        if self.config.enforce {
            decision
        } else {
            PerSkillDecision::Shadow {
                would_be: Box::new(decision),
            }
        }
    }

    fn resolve(&self, req: &PerSkillRequest) -> PerSkillDecision {
        let candidates = [
            SkillScope::Skill {
                tenant: req.tenant.clone(),
                agent: req.agent.clone(),
                skill: req.skill.clone(),
            },
            SkillScope::Agent {
                tenant: req.tenant.clone(),
                agent: req.agent.clone(),
            },
            SkillScope::Tenant {
                tenant: req.tenant.clone(),
            },
        ];

        let mut best: Option<ResolvedRule> = None;
        for scope in candidates {
            if let Some(rule) = self.rules.get(&scope) {
                let resolved = ResolvedRule {
                    scope: scope.clone(),
                    rule: rule.clone(),
                };
                match &best {
                    Some(b) if b.scope.specificity() >= resolved.scope.specificity() => {}
                    _ => best = Some(resolved),
                }
            }
        }

        match best {
            Some(r) => match r.rule.effect {
                SkillEffect::Allow => PerSkillDecision::Allow { contributor: Some(r) },
                SkillEffect::Deny => PerSkillDecision::Deny {
                    reason: r.rule.reason.clone(),
                    contributor: Some(r),
                },
            },
            None => {
                if self.config.default_deny_when_no_bundles {
                    PerSkillDecision::Deny {
                        reason: "no_policy".into(),
                        contributor: None,
                    }
                } else {
                    PerSkillDecision::Allow { contributor: None }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
