use std::sync::Arc;
use std::time::SystemTime;

use a2a_auth_callout::SpiceDbSubject;
use a2a_nats::A2aAgentId;
use a2a_nats::server::A2aMethod;
use trogon_std::env::ReadEnv;

use super::bundle::{
    Tier1DeclarativeBundle, Tier1DeclarativeDecision, Tier1DeclarativeEffect, Tier1DeclarativeMatch,
    Tier1DeclarativeRule, Tier1ResourceKind,
};
use super::loader::Tier1DeclarativeLoadError;
use super::time_predicate::time_of_day_pattern_matches;

pub trait Tier1Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Default)]
pub struct SystemTier1Clock;

impl Tier1Clock for SystemTier1Clock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Debug, Clone)]
pub struct FixedTier1Clock(SystemTime);

impl FixedTier1Clock {
    pub fn new(instant: SystemTime) -> Self {
        Self(instant)
    }
}

impl Tier1Clock for FixedTier1Clock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

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
    clock: Arc<dyn Tier1Clock>,
}

impl RealTier1DeclarativeGate {
    pub fn new(bundle: Tier1DeclarativeBundle) -> Self {
        Self::with_clock(bundle, Arc::new(SystemTier1Clock))
    }

    pub fn with_clock(bundle: Tier1DeclarativeBundle, clock: Arc<dyn Tier1Clock>) -> Self {
        Self { bundle, clock }
    }
}

impl Tier1DeclarativeGate for RealTier1DeclarativeGate {
    fn is_enabled(&self) -> bool {
        true
    }

    fn evaluate(&self, ctx: &Tier1DeclarativeContext) -> Tier1DeclarativeDecision {
        for rule in self.bundle.rules() {
            if rule_matches_all(ctx, rule, self.clock.as_ref()) {
                return match rule.effect {
                    Tier1DeclarativeEffect::Allow => Tier1DeclarativeDecision::Allow {
                        rule: Some(rule.id.clone()),
                    },
                    Tier1DeclarativeEffect::Deny => Tier1DeclarativeDecision::Deny { rule: rule.id.clone() },
                };
            }
        }

        Tier1DeclarativeDecision::Allow { rule: None }
    }
}

fn rule_matches_all(ctx: &Tier1DeclarativeContext, rule: &Tier1DeclarativeRule, clock: &dyn Tier1Clock) -> bool {
    rule.matches.iter().all(|item| match_hits(ctx, item, clock))
}

fn match_hits(ctx: &Tier1DeclarativeContext, item: &Tier1DeclarativeMatch, clock: &dyn Tier1Clock) -> bool {
    let matched = match item.kind {
        Tier1ResourceKind::TimeOfDay => time_of_day_pattern_matches(&item.pattern, clock.now()).unwrap_or(false),
        kind => {
            let value = field_value(ctx, kind);
            pattern_matches(&item.pattern, &value)
        }
    };
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
        Tier1ResourceKind::TimeOfDay => String::new(),
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1DeclarativeBuildError {
    #[error("invalid tier-1 declarative bundle dir: {0}")]
    InvalidBundleDir(String),
    #[error("{ENV_TIER1_DECLARATIVE_ENABLED}=on requires {ENV_TIER1_BUNDLE_DIR}")]
    MissingBundleDir,
    #[error(transparent)]
    Load(#[from] Tier1DeclarativeLoadError),
}

pub struct GatewayTier1DeclarativeLayer {
    pub gate: Arc<dyn Tier1DeclarativeGate>,
}

impl std::fmt::Debug for GatewayTier1DeclarativeLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayTier1DeclarativeLayer")
            .field("enabled", &self.gate.is_enabled())
            .finish()
    }
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
        Tier1DeclarativeDecision::Allow { rule: None } => "gateway.tier1.declarative.no_match_default_allow".into(),
        Tier1DeclarativeDecision::Allow { rule: Some(id) } => {
            format!("gateway.tier1.declarative.allowed.{}", id.as_str())
        }
        Tier1DeclarativeDecision::Deny { rule } => {
            format!("gateway.tier1.declarative.denied.{}", rule.as_str())
        }
    }
}

#[cfg(test)]
mod tests;
