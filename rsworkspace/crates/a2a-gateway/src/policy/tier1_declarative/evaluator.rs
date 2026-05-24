use std::fmt;
use std::sync::Arc;

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::A2aMethod;
use trogon_std::env::ReadEnv;

use super::bundle::{
    Tier1DeclarativeBundle, Tier1DeclarativeDecision, Tier1DeclarativeEffect, Tier1DeclarativeMatch,
    Tier1DeclarativeRule, Tier1ResourceKind,
};
use super::loader::Tier1DeclarativeLoadError;

pub const ENV_TIER1_DECLARATIVE_ENABLED: &str = "A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED";
pub const ENV_TIER1_BUNDLE_DIR: &str = "A2A_GATEWAY_TIER1_BUNDLE_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1DeclarativeContext {
    pub agent_method: A2aMethod,
    pub agent_id: A2aAgentId,
    pub caller_subject: Option<SpiceDbSubject>,
    pub nats_subject: String,
}

impl Tier1DeclarativeContext {
    pub fn new(
        agent_method: A2aMethod,
        agent_id: A2aAgentId,
        caller_subject: Option<SpiceDbSubject>,
        nats_subject: impl Into<String>,
    ) -> Self {
        Self {
            agent_method,
            agent_id,
            caller_subject,
            nats_subject: nats_subject.into(),
        }
    }
}

pub trait Tier1DeclarativeGate: Send + Sync {
    fn is_enabled(&self) -> bool;

    fn evaluate(&self, ctx: &Tier1DeclarativeContext) -> Tier1DeclarativeDecision;
}

#[derive(Debug, Default)]
pub struct NoopTier1DeclarativeGate;

impl Tier1DeclarativeGate for NoopTier1DeclarativeGate {
    fn is_enabled(&self) -> bool {
        false
    }

    fn evaluate(&self, _ctx: &Tier1DeclarativeContext) -> Tier1DeclarativeDecision {
        Tier1DeclarativeDecision::Allow { rule: None }
    }
}

pub struct RealTier1DeclarativeGate {
    bundle: Tier1DeclarativeBundle,
}

impl RealTier1DeclarativeGate {
    pub fn new(bundle: Tier1DeclarativeBundle) -> Self {
        Self { bundle }
    }
}

impl Tier1DeclarativeGate for RealTier1DeclarativeGate {
    fn is_enabled(&self) -> bool {
        true
    }

    fn evaluate(&self, ctx: &Tier1DeclarativeContext) -> Tier1DeclarativeDecision {
        for rule in self.bundle.rules() {
            if rule_matches_all(ctx, rule) {
                return match rule.effect {
                    Tier1DeclarativeEffect::Allow => Tier1DeclarativeDecision::Allow {
                        rule: Some(rule.id.clone()),
                    },
                    Tier1DeclarativeEffect::Deny => Tier1DeclarativeDecision::Deny {
                        rule: rule.id.clone(),
                    },
                };
            }
        }

        Tier1DeclarativeDecision::Allow { rule: None }
    }
}

fn rule_matches_all(ctx: &Tier1DeclarativeContext, rule: &Tier1DeclarativeRule) -> bool {
    rule.matches.iter().all(|item| match_hits(ctx, item))
}

fn match_hits(ctx: &Tier1DeclarativeContext, item: &Tier1DeclarativeMatch) -> bool {
    let value = field_value(ctx, item.kind);
    let matched = pattern_matches(&item.pattern, &value);
    if item.negate { !matched } else { matched }
}

fn field_value(ctx: &Tier1DeclarativeContext, kind: Tier1ResourceKind) -> String {
    match kind {
        Tier1ResourceKind::AgentMethod => ctx.agent_method.as_str().to_owned(),
        Tier1ResourceKind::AgentId => ctx.agent_id.as_str().to_owned(),
        Tier1ResourceKind::CallerSubject => ctx
            .caller_subject
            .as_ref()
            .map(SpiceDbSubject::as_str)
            .unwrap_or_default()
            .to_owned(),
        Tier1ResourceKind::NatsSubjectPattern => ctx.nats_subject.clone(),
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == value;
    }
    glob_match(pattern.as_bytes(), value.as_bytes())
}

fn glob_match(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }

    if pattern[0] == b'*' {
        if pattern.len() == 1 {
            return true;
        }
        for index in 0..=value.len() {
            if glob_match(&pattern[1..], &value[index..]) {
                return true;
            }
        }
        return false;
    }

    if value.is_empty() {
        return false;
    }

    if pattern[0] == value[0] && glob_match(&pattern[1..], &value[1..]) {
        return true;
    }

    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1DeclarativeBuildError {
    InvalidBundleDir(String),
    MissingBundleDir,
    Load(Tier1DeclarativeLoadError),
}

impl fmt::Display for Tier1DeclarativeBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBundleDir(path) => write!(f, "invalid tier-1 declarative bundle dir: {path}"),
            Self::MissingBundleDir => write!(
                f,
                "{ENV_TIER1_DECLARATIVE_ENABLED}=on requires {ENV_TIER1_BUNDLE_DIR}"
            ),
            Self::Load(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Tier1DeclarativeBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(error) => Some(error),
            _ => None,
        }
    }
}

pub struct GatewayTier1DeclarativeLayer {
    pub gate: Arc<dyn Tier1DeclarativeGate>,
}

pub struct Tier1DeclarativeConfig;

impl Tier1DeclarativeConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Result<GatewayTier1DeclarativeLayer, Tier1DeclarativeBuildError> {
        if !tier1_declarative_enabled(env) {
            return Ok(GatewayTier1DeclarativeLayer {
                gate: Arc::new(NoopTier1DeclarativeGate),
            });
        }

        let bundle_dir = match env.var(ENV_TIER1_BUNDLE_DIR) {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) => return Err(Tier1DeclarativeBuildError::InvalidBundleDir(String::new())),
            Err(std::env::VarError::NotPresent) => return Err(Tier1DeclarativeBuildError::MissingBundleDir),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Tier1DeclarativeBuildError::InvalidBundleDir(
                    ENV_TIER1_BUNDLE_DIR.into(),
                ));
            }
        };

        let bundle = Tier1DeclarativeBundle::load_from_dir(&bundle_dir).map_err(Tier1DeclarativeBuildError::Load)?;
        Ok(GatewayTier1DeclarativeLayer {
            gate: Arc::new(RealTier1DeclarativeGate::new(bundle)),
        })
    }
}

fn tier1_declarative_enabled<E: ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER1_DECLARATIVE_ENABLED) {
        Ok(raw) => matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => false,
    }
}

pub fn tier1_declarative_audit_rule_fired(decision: &Tier1DeclarativeDecision) -> String {
    match decision {
        Tier1DeclarativeDecision::Allow { rule: None } => {
            "gateway.tier1.declarative.no_match_default_allow".into()
        }
        Tier1DeclarativeDecision::Allow { rule: Some(id) } => {
            format!("gateway.tier1.declarative.allowed.{}", id.as_str())
        }
        Tier1DeclarativeDecision::Deny { rule } => {
            format!("gateway.tier1.declarative.denied.{}", rule.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use a2a_auth_callout::SpiceDbSubject;

    use super::*;
    use crate::policy::tier1_declarative::bundle::{
        Tier1DeclarativeEffect, Tier1DeclarativeMatch, Tier1DeclarativeRule, Tier1DeclarativeRuleId,
        Tier1ResourceKind,
    };

    fn ctx(method: A2aMethod, agent: &str, caller: Option<&str>, subject: &str) -> Tier1DeclarativeContext {
        Tier1DeclarativeContext::new(
            method,
            A2aAgentId::new(agent).expect("agent id"),
            caller.map(SpiceDbSubject::new),
            subject,
        )
    }

    fn rule(id: &str, priority: u32, effect: Tier1DeclarativeEffect, matches: Vec<Tier1DeclarativeMatch>) -> Tier1DeclarativeRule {
        Tier1DeclarativeRule {
            id: Tier1DeclarativeRuleId::new(id),
            matches,
            effect,
            priority,
        }
    }

    #[test]
    fn matching_rule_returns_effect() {
        let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![rule(
            "deny-planner",
            100,
            Tier1DeclarativeEffect::Deny,
            vec![
                Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentId, "planner", false),
                Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentMethod, "message/send", false),
            ],
        )]));

        let decision = gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            Some("user/alice"),
            "a2a.gateway.planner.message.send",
        ));
        assert_eq!(
            decision,
            Tier1DeclarativeDecision::Deny {
                rule: Tier1DeclarativeRuleId::new("deny-planner")
            }
        );
    }

    #[test]
    fn partial_match_tries_next_rule() {
        let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![
            rule(
                "deny-planner",
                100,
                Tier1DeclarativeEffect::Deny,
                vec![Tier1DeclarativeMatch::new(
                    Tier1ResourceKind::AgentId,
                    "other",
                    false,
                )],
            ),
            rule(
                "allow-planner",
                50,
                Tier1DeclarativeEffect::Allow,
                vec![Tier1DeclarativeMatch::new(
                    Tier1ResourceKind::AgentId,
                    "planner",
                    false,
                )],
            ),
        ]));

        let decision = gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            None,
            "a2a.gateway.planner.message.send",
        ));
        assert_eq!(
            decision,
            Tier1DeclarativeDecision::Allow {
                rule: Some(Tier1DeclarativeRuleId::new("allow-planner"))
            }
        );
    }

    #[test]
    fn priority_order_is_respected() {
        let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![
            rule(
                "allow-low",
                10,
                Tier1DeclarativeEffect::Allow,
                vec![Tier1DeclarativeMatch::new(
                    Tier1ResourceKind::AgentId,
                    "planner",
                    false,
                )],
            ),
            rule(
                "deny-high",
                100,
                Tier1DeclarativeEffect::Deny,
                vec![Tier1DeclarativeMatch::new(
                    Tier1ResourceKind::AgentId,
                    "planner",
                    false,
                )],
            ),
        ]));

        let decision = gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            None,
            "a2a.gateway.planner.message.send",
        ));
        assert_eq!(
            decision,
            Tier1DeclarativeDecision::Deny {
                rule: Tier1DeclarativeRuleId::new("deny-high")
            }
        );
    }

    #[test]
    fn exact_and_glob_patterns_work() {
        assert!(pattern_matches("message/send", "message/send"));
        assert!(!pattern_matches("message/send", "message/stream"));
        assert!(pattern_matches("user/*", "user/alice"));
        assert!(pattern_matches("a2a.gateway.*.message.send", "a2a.gateway.planner.message.send"));
        assert!(!pattern_matches("a2a.gateway.*.message.send", "a2a.gateway.planner.message.stream"));
    }

    #[test]
    fn negate_inverts_match() {
        let gate = RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![rule(
            "deny-non-alice",
            100,
            Tier1DeclarativeEffect::Deny,
            vec![Tier1DeclarativeMatch::new(
                Tier1ResourceKind::CallerSubject,
                "user/alice",
                true,
            )],
        )]));

        let denied = gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            Some("user/bob"),
            "a2a.gateway.planner.message.send",
        ));
        assert_eq!(
            denied,
            Tier1DeclarativeDecision::Deny {
                rule: Tier1DeclarativeRuleId::new("deny-non-alice")
            }
        );

        let allowed = gate.evaluate(&ctx(
            A2aMethod::MessageSend,
            "planner",
            Some("user/alice"),
            "a2a.gateway.planner.message.send",
        ));
        assert_eq!(allowed, Tier1DeclarativeDecision::Allow { rule: None });
    }

    #[test]
    fn noop_gate_defaults_allow() {
        let gate = NoopTier1DeclarativeGate;
        assert!(!gate.is_enabled());
        assert_eq!(
            gate.evaluate(&ctx(
                A2aMethod::MessageSend,
                "planner",
                None,
                "a2a.gateway.planner.message.send",
            )),
            Tier1DeclarativeDecision::Allow { rule: None }
        );
    }
}
