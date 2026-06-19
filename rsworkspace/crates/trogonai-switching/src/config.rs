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

/// § Checkpoint strictness — configurable risk policy for whether a continuity
/// checkpoint validates against the REAL destination runner/model or echoes the Context
/// Twin internally (§1986: "switches con tools, artifacts, terminal o cambios de
/// proveedor deben usar checkpoint real contra destino"; §2280: `internal_echo` is valid
/// only for tests/shadow/MVP). Default = `InternalEcho` (current behavior, no regression);
/// production sets `RealForHighRisk` so high-risk switches escalate to a real checkpoint
/// even when the global echo default is on. Change criterion / rollback: flip the env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckpointStrictness {
    /// MVP / shadow / tests: the legacy `continuity_checkpoint_internal_echo` flag governs
    /// echo-vs-real for ALL switches (echo by default; explicitly off ⇒ real for all).
    #[default]
    InternalEcho,
    /// Production: HIGH-RISK switches (provider change, tools, artifacts, live terminal)
    /// validate against the real destination; low-risk switches may echo.
    RealForHighRisk,
    /// Every checkpoint validates against the real destination.
    AlwaysReal,
}

impl CheckpointStrictness {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "real_for_high_risk" | "high_risk" | "high-risk" => Self::RealForHighRisk,
            "always_real" | "always" | "real" => Self::AlwaysReal,
            _ => Self::InternalEcho,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InternalEcho => "internal_echo",
            Self::RealForHighRisk => "real_for_high_risk",
            Self::AlwaysReal => "always_real",
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

    /// § Checkpoint strictness. Raw value parsed via [`SwitchingConfig::checkpoint_strictness`].
    /// Default `internal_echo` preserves current behavior (echo governed by the legacy flag).
    #[config(env = "TROGON_SWITCHING_CHECKPOINT_STRICTNESS", default = "real_for_high_risk")]
    pub checkpoint_strictness_raw: String,
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

    /// Configurable risk policy for checkpoint strictness (§ Checkpoint strictness).
    pub fn checkpoint_strictness(&self) -> CheckpointStrictness {
        CheckpointStrictness::parse(&self.checkpoint_strictness_raw)
    }

    /// Programmatically set the checkpoint strictness policy.
    pub fn with_checkpoint_strictness(mut self, strictness: CheckpointStrictness) -> Self {
        self.checkpoint_strictness_raw = strictness.as_str().to_string();
        self
    }

    /// Whether a continuity checkpoint must validate against the REAL destination
    /// runner/model (vs a Context-Twin echo), given the switch's risk (§ Checkpoint
    /// strictness, §2280). Real for `AlwaysReal`; for `RealForHighRisk` only when the
    /// switch is high-risk; under `InternalEcho` echo unless the legacy
    /// `continuity_checkpoint_internal_echo` flag is explicitly off (real for all).
    pub fn requires_real_checkpoint(&self, is_high_risk: bool) -> bool {
        match self.checkpoint_strictness() {
            CheckpointStrictness::AlwaysReal => true,
            CheckpointStrictness::RealForHighRisk => is_high_risk,
            CheckpointStrictness::InternalEcho => !self.continuity_checkpoint_internal_echo,
        }
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
        // Default strictness escalates high-risk switches to a real destination checkpoint.
        assert_eq!(config.checkpoint_strictness(), CheckpointStrictness::RealForHighRisk);
        assert!(config.continuity_checkpoint_internal_echo);
        assert!(
            config.requires_real_checkpoint(true),
            "default uses real checkpoint for high-risk switches"
        );
        assert!(
            !config.requires_real_checkpoint(false),
            "low-risk switches may still echo by default"
        );
    }

    #[test]
    fn requires_real_checkpoint_follows_strictness_and_risk() {
        // § Checkpoint strictness (§1986/§2280): the real-vs-echo decision is risk-based.
        // InternalEcho (default) + echo on → echo for all.
        let echo = SwitchingConfig::default().with_checkpoint_strictness(CheckpointStrictness::InternalEcho);
        assert!(!echo.requires_real_checkpoint(true));
        assert!(!echo.requires_real_checkpoint(false));

        // RealForHighRisk → real ONLY for high-risk switches, even with the echo default on.
        let risk = SwitchingConfig::default().with_checkpoint_strictness(CheckpointStrictness::RealForHighRisk);
        assert!(risk.continuity_checkpoint_internal_echo, "global echo default still on");
        assert!(
            risk.requires_real_checkpoint(true),
            "high-risk escalates to a real checkpoint"
        );
        assert!(!risk.requires_real_checkpoint(false), "low-risk may still echo");

        // AlwaysReal → real for every switch.
        let always = SwitchingConfig::default().with_checkpoint_strictness(CheckpointStrictness::AlwaysReal);
        assert!(always.requires_real_checkpoint(true));
        assert!(always.requires_real_checkpoint(false));

        // Legacy back-compat: InternalEcho with the echo flag explicitly off ⇒ real for all.
        let mut legacy_real = SwitchingConfig::default().with_checkpoint_strictness(CheckpointStrictness::InternalEcho);
        legacy_real.continuity_checkpoint_internal_echo = false;
        assert!(legacy_real.requires_real_checkpoint(false));
        assert!(legacy_real.requires_real_checkpoint(true));
    }

    #[test]
    fn checkpoint_strictness_parse_roundtrips() {
        for s in [
            CheckpointStrictness::InternalEcho,
            CheckpointStrictness::RealForHighRisk,
            CheckpointStrictness::AlwaysReal,
        ] {
            assert_eq!(CheckpointStrictness::parse(s.as_str()), s);
        }
        assert_eq!(
            CheckpointStrictness::parse("nonsense"),
            CheckpointStrictness::InternalEcho
        );
    }
}
