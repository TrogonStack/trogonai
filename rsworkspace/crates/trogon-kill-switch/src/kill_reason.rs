use std::fmt;

/// Why an agent was killed.
///
/// AGT's spec lists six reasons (`behavioral_drift`, `rate_limit`,
/// `ring_breach`, `manual`, `quarantine_timeout`, `session_timeout`). This
/// type carries the four reasons that are structural to a governance
/// decision made *about* an agent from the outside: a policy layer
/// observing drift, rate limits, or a ring breach, or an operator acting
/// directly. `quarantine_timeout` and `session_timeout` are AGT session
/// lifecycle concepts with no equivalent primitive in this workspace yet;
/// they are intentionally left out rather than stubbed, and can be added
/// as variants (an additive, non-breaking change to this enum's callers
/// via exhaustive `match`) if a `trogon-*` session/quarantine model is
/// ever built. See `docs/proposals/kill-switch-enforcement.md` for the
/// full provenance discussion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KillReason {
    /// Agent behavior diverged from expected patterns.
    BehavioralDrift,
    /// Persistent rate limit violations.
    RateLimit,
    /// Attempted unauthorized ring/subject access.
    RingBreach,
    /// Operator-initiated kill.
    Manual,
}

impl KillReason {
    /// Stable wire name, matching AGT's `KillReason` string enum values so
    /// audit tooling that already understands AGT's taxonomy reads ours
    /// without a translation table.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BehavioralDrift => "behavioral_drift",
            Self::RateLimit => "rate_limit",
            Self::RingBreach => "ring_breach",
            Self::Manual => "manual",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, KillReasonError> {
        match raw {
            "behavioral_drift" => Ok(Self::BehavioralDrift),
            "rate_limit" => Ok(Self::RateLimit),
            "ring_breach" => Ok(Self::RingBreach),
            "manual" => Ok(Self::Manual),
            _ => Err(KillReasonError::Unknown { raw: raw.to_string() }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KillReasonError {
    #[error("unknown kill reason: '{raw}'")]
    Unknown { raw: String },
}

impl fmt::Display for KillReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
