use std::time::Duration;

use confique::Config;

/// Default NATS namespace prefix for runner binding KV.
pub const DEFAULT_NATS_PREFIX: &str = "ACP";
/// Minimum confidence for continuity checkpoint acknowledgement.
pub const DEFAULT_CHECKPOINT_MIN_CONFIDENCE: f64 = 0.75;
/// Mismatch ratio above which checkpoint is considered failed.
pub const DEFAULT_CHECKPOINT_MISMATCH_THRESHOLD: f64 = 0.35;

/// § Block vs confirmation — configurable risk policy (§ "matriz por riesgo para
/// decidir si una degradacion bloquea o requiere confirmacion"). This ONLY governs a
/// *degradation* (a portable capability loss that the gate would otherwise allow with a
/// warning). The CLOSED rule is untouched: irreversible / non-portable / indispensable
/// loss always blocks regardless of profile (§ "Configurable no significa abierto. La
/// regla de producto esta cerrada"). Default = `Warn` (current behavior, no regression);
/// change criterion: tighten to `Confirm`/`Block` per provider risk once shadow data
/// justifies it; rollback = reset the env var to `warn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DegradationPolicy {
    /// Proceed with a recorded warning (AllowedWithWarning) — the most permissive choice.
    Warn,
    /// Require explicit user confirmation for any degradation. Default: matches the closed
    /// rule "degradacion aceptable requiere confirmacion explicita" (§ Block vs confirmation).
    #[default]
    Confirm,
    /// Block the switch until the degradation is resolved — the strictest risk choice.
    Block,
}

impl DegradationPolicy {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "confirm" | "confirmation" | "require_confirmation" => Self::Confirm,
            "block" | "blocked" => Self::Block,
            _ => Self::Warn,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Confirm => "confirm",
            Self::Block => "block",
        }
    }
}

/// Switch orchestration configuration loaded via ADR 0007 precedence.
#[derive(Config, Clone, Debug, PartialEq)]
pub struct SwitchingConfig {
    #[config(env = "TROGON_SWITCHING_NATS_PREFIX", default = "ACP")]
    pub nats_prefix: String,

    #[config(env = "TROGON_SWITCHING_SAFETY_GATE_ENABLED", default = true)]
    pub switch_safety_gate_enabled: bool,

    #[config(env = "TROGON_SWITCHING_CONTINUITY_CHECKPOINT_ENABLED", default = true)]
    pub continuity_checkpoint_enabled: bool,

    #[config(env = "TROGON_SWITCHING_CHECKPOINT_MIN_CONFIDENCE", default = 0.75)]
    pub checkpoint_min_confidence: f64,

    #[config(env = "TROGON_SWITCHING_CHECKPOINT_MISMATCH_THRESHOLD", default = 0.35)]
    pub checkpoint_mismatch_threshold: f64,

    #[config(env = "TROGON_SWITCHING_LONG_SESSION_TURN_THRESHOLD", default = 24)]
    pub long_session_turn_threshold: usize,

    #[config(env = "TROGON_SWITCHING_INLINE_ARTIFACT_LIMIT_BYTES", default = 65536)]
    pub inline_artifact_limit_bytes: usize,

    /// When true, continuity checkpoint echoes Context Twin internally instead of
    /// calling the target runner (MVP / shadow rollout).
    #[config(env = "TROGON_SWITCHING_CHECKPOINT_INTERNAL_ECHO", default = true)]
    pub continuity_checkpoint_internal_echo: bool,

    /// § Block vs confirmation matrix. Raw value parsed via [`SwitchingConfig::degradation_policy`].
    /// Default `confirm` keeps the closed rule (degradation requires explicit confirmation).
    #[config(env = "TROGON_SWITCHING_DEGRADATION_POLICY", default = "confirm")]
    pub degradation_policy_raw: String,
}

impl SwitchingConfig {
    pub fn checkpoint_latency_budget(&self) -> Duration {
        Duration::from_secs(20)
    }

    pub fn switch_latency_budget(&self) -> Duration {
        Duration::from_secs(5)
    }

    /// Configurable risk policy for how a degradation is surfaced (§ Block vs confirmation).
    pub fn degradation_policy(&self) -> DegradationPolicy {
        DegradationPolicy::parse(&self.degradation_policy_raw)
    }

    /// Programmatically set the degradation risk policy (the raw field is always parsed
    /// through [`DegradationPolicy`]).
    pub fn with_degradation_policy(mut self, policy: DegradationPolicy) -> Self {
        self.degradation_policy_raw = policy.as_str().to_string();
        self
    }
}

impl Default for SwitchingConfig {
    fn default() -> Self {
        Self::builder().load().expect("switching config defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_values() {
        let config = SwitchingConfig::default();
        assert_eq!(config.nats_prefix, DEFAULT_NATS_PREFIX);
        assert!(config.switch_safety_gate_enabled);
        assert!(config.continuity_checkpoint_enabled);
        assert_eq!(config.checkpoint_min_confidence, DEFAULT_CHECKPOINT_MIN_CONFIDENCE);
    }
}
