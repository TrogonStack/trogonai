//! Tier-1 declarative bundle schema (`*.tier1.toml` under `A2A_GATEWAY_TIER1_BUNDLE_DIR`).
//!
//! ```toml
//! [[rule]]
//! id = "deny-guest-planner"
//! priority = 100
//! effect = "deny"
//!
//! [[rule.matches]]
//! kind = "agent_id"
//! pattern = "planner"
//!
//! [[rule.matches]]
//! kind = "agent_method"
//! pattern = "message/send"
//! negate = false
//!
//! [[rule.matches]]
//! kind = "caller_subject"
//! pattern = "user/guest-*"
//!
//! [[rule.matches]]
//! kind = "nats_subject_pattern"
//! pattern = "a2a.gateway.*.message.send"
//! ```
//!
//! `kind` is one of `agent_method`, `agent_id`, `caller_subject`, `nats_subject_pattern`.
//! `effect` is `allow` or `deny`. All `matches` on a rule must hit (AND). Rules are ordered by
//! descending `priority`; the first fully matching rule wins. Unmatched requests default to allow.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tier1DeclarativeRuleId(String);

impl Tier1DeclarativeRuleId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Tier1DeclarativeRuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier1ResourceKind {
    AgentMethod,
    AgentId,
    CallerSubject,
    NatsSubjectPattern,
}

impl Tier1ResourceKind {
    pub fn parse(raw: &str) -> Result<Self, Tier1DeclarativeSchemaError> {
        match raw {
            "agent_method" => Ok(Self::AgentMethod),
            "agent_id" => Ok(Self::AgentId),
            "caller_subject" => Ok(Self::CallerSubject),
            "nats_subject_pattern" => Ok(Self::NatsSubjectPattern),
            other => Err(Tier1DeclarativeSchemaError::UnknownKind(other.into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1DeclarativeMatch {
    pub kind: Tier1ResourceKind,
    pub pattern: String,
    pub negate: bool,
}

impl Tier1DeclarativeMatch {
    pub fn new(kind: Tier1ResourceKind, pattern: impl Into<String>, negate: bool) -> Self {
        Self {
            kind,
            pattern: pattern.into(),
            negate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier1DeclarativeEffect {
    Allow,
    Deny,
}

impl Tier1DeclarativeEffect {
    pub fn parse(raw: &str) -> Result<Self, Tier1DeclarativeSchemaError> {
        match raw {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            other => Err(Tier1DeclarativeSchemaError::UnknownEffect(other.into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1DeclarativeRule {
    pub id: Tier1DeclarativeRuleId,
    pub matches: Vec<Tier1DeclarativeMatch>,
    pub effect: Tier1DeclarativeEffect,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1DeclarativeBundle {
    rules: Vec<Tier1DeclarativeRule>,
}

impl Tier1DeclarativeBundle {
    pub fn new(mut rules: Vec<Tier1DeclarativeRule>) -> Self {
        rules.sort_by(|left, right| right.priority.cmp(&left.priority));
        Self { rules }
    }

    pub fn rules(&self) -> &[Tier1DeclarativeRule] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1DeclarativeDecision {
    Allow {
        rule: Option<Tier1DeclarativeRuleId>,
    },
    Deny {
        rule: Tier1DeclarativeRuleId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1DeclarativeSchemaError {
    EmptyRuleId,
    EmptyPattern,
    UnknownKind(String),
    UnknownEffect(String),
}

impl fmt::Display for Tier1DeclarativeSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRuleId => write!(f, "rule id must not be empty"),
            Self::EmptyPattern => write!(f, "match pattern must not be empty"),
            Self::UnknownKind(kind) => write!(f, "unknown match kind `{kind}`"),
            Self::UnknownEffect(effect) => write!(f, "unknown rule effect `{effect}`"),
        }
    }
}

impl std::error::Error for Tier1DeclarativeSchemaError {}
