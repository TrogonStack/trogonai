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
//!
//! [[rule.matches]]
//! kind = "time_of_day"
//! pattern = "Mon-Fri|09:00-17:00|UTC"
//! ```
//!
//! `kind` is one of `agent_method`, `agent_id`, `caller_subject`,
//! `nats_subject_pattern`, or `time_of_day`. `effect` is `allow` or `deny`.
//! All `matches` on a rule must hit (AND). Rules are ordered by descending
//! `priority`, then by ascending `id` for a stable tie-breaker. The first
//! fully matching rule wins. Unmatched requests default to allow.

use std::fmt;

use super::time_predicate::{TimeOfDayParseError, TimeOfDayWindow};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tier1DeclarativeRuleId(String);

impl Tier1DeclarativeRuleId {
    /// Construct a validated rule id. Rejects empty / whitespace-only ids
    /// at construction so an invalid `Tier1DeclarativeRuleId` value can't
    /// exist in memory; the loader and any direct caller share the same
    /// validation path.
    pub fn new(id: impl Into<String>) -> Result<Self, Tier1DeclarativeSchemaError> {
        let raw = id.into();
        if raw.trim().is_empty() {
            return Err(Tier1DeclarativeSchemaError::EmptyRuleId);
        }
        Ok(Self(raw))
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
    TimeOfDay,
}

impl Tier1ResourceKind {
    pub fn parse(raw: &str) -> Result<Self, Tier1DeclarativeSchemaError> {
        match raw {
            "agent_method" => Ok(Self::AgentMethod),
            "agent_id" => Ok(Self::AgentId),
            "caller_subject" => Ok(Self::CallerSubject),
            "nats_subject_pattern" => Ok(Self::NatsSubjectPattern),
            "time_of_day" => Ok(Self::TimeOfDay),
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
    /// Construct a validated match. Rejects empty patterns and, for
    /// `TimeOfDay` matches, requires the pattern to parse — so invalid
    /// matches can't reach the evaluator regardless of how they're built.
    pub fn new(
        kind: Tier1ResourceKind,
        pattern: impl Into<String>,
        negate: bool,
    ) -> Result<Self, Tier1DeclarativeSchemaError> {
        let pattern = pattern.into();
        if pattern.trim().is_empty() {
            return Err(Tier1DeclarativeSchemaError::EmptyPattern);
        }
        if kind == Tier1ResourceKind::TimeOfDay {
            TimeOfDayWindow::parse(pattern.trim()).map_err(Tier1DeclarativeSchemaError::InvalidTimeOfDayPattern)?;
        }
        Ok(Self { kind, pattern, negate })
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
        // Primary sort: priority descending (high priority wins first).
        // Secondary sort: rule id ascending — without a deterministic
        // tie-breaker, equal-priority rules would resolve based on
        // `read_dir` order, which differs by filesystem and across
        // restarts; ingress decisions would flip for identical traffic.
        rules.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.id.as_str().cmp(right.id.as_str()))
        });
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
    Allow { rule: Option<Tier1DeclarativeRuleId> },
    Deny { rule: Tier1DeclarativeRuleId },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1DeclarativeSchemaError {
    #[error("rule id must not be empty")]
    EmptyRuleId,
    #[error("match pattern must not be empty")]
    EmptyPattern,
    #[error("unknown match kind `{0}`")]
    UnknownKind(String),
    #[error("unknown rule effect `{0}`")]
    UnknownEffect(String),
    /// Carries the typed `TimeOfDayParseError` so callers can route on the
    /// specific parse failure (empty, segment count, weekday, time
    /// window, timezone) instead of pattern-matching error strings.
    #[error("invalid time_of_day pattern")]
    InvalidTimeOfDayPattern(#[from] TimeOfDayParseError),
    #[error(
        "rule `{0}` must declare at least one match — a rule with no `[[rule.matches]]` would fire on every request"
    )]
    NoMatches(String),
}
