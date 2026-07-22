//! Rollout promotion/rollback enforcement (§ "Implementar enforcement de promocion/
//! rollback basada en metricas"; § Decisiones finales cerradas, §2240: "ambos umbrales
//! deben existir antes de activar default").
//!
//! [`crate::promotion::evaluate_promotion_readiness`] is the pure gate: it decides whether
//! the measured metrics are healthy. This module turns that decision into the ACTION the
//! doc requires — when a threshold is breached, the effective feature flags are forced
//! back to the conservative defaults (`runner_binding_mode=handoff`,
//! `event_log_primary_mode=legacy_messages`), i.e. the automatic rollback that prevents
//! canonical/event-primary from staying promoted while rollout health is bad.

use crate::features::SessionKernelFeatureFlags;
use crate::migration::ShadowDivergenceReport;
use crate::policies::RolloutPromotionPolicy;
use crate::promotion::{PromotionBlockReason, PromotionReadiness, evaluate_promotion_readiness};
use crate::telemetry;

/// Measured rollout health fed to the enforcement gate (§ Fallback budget / Shadow
/// divergence). These are the metrics the rollout must observe before promoting canonical
/// or event-primary to default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RolloutMetrics {
    /// Canonical replay-vs-legacy divergence (zero canonical loss tolerated).
    pub divergence: ShadowDivergenceReport,
    /// Fallbacks to handoff observed in canonical/kernel-primary sessions.
    pub canonical_fallback_count: usize,
    /// Recorded minor projection-only divergences (allowed only under budget).
    pub minor_projection_divergences: usize,
}

impl RolloutMetrics {
    /// Clean baseline: no divergence and no fallbacks. Enforcement against this is always
    /// a no-op (`Promote`), used when no adverse metrics have been measured yet.
    pub fn clean() -> Self {
        Self::default()
    }
}

/// Action taken by rollout enforcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RolloutDecision {
    /// Metrics are healthy: canonical/event-primary may remain or be promoted.
    Promote,
    /// A threshold was breached: roll back to the conservative defaults.
    RollBack(Vec<PromotionBlockReason>),
}

impl RolloutDecision {
    pub fn is_rollback(&self) -> bool {
        matches!(self, RolloutDecision::RollBack(_))
    }
}

/// Outcome of enforcing rollout policy against measured metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutEnforcementOutcome {
    pub decision: RolloutDecision,
    /// The flags that must be in effect after enforcement. On rollback these are forced to
    /// the conservative defaults regardless of the configured values; on promote they are
    /// the input flags unchanged.
    pub effective_flags: SessionKernelFeatureFlags,
}

/// Enforce rollout promotion/rollback against measured metrics.
///
/// Healthy metrics leave `flags` unchanged (`Promote`). Any breached threshold forces the
/// effective flags back to conservative defaults (`handoff` + `legacy_messages`) and
/// reports the reasons (`RollBack`) — the automatic rollback the doc mandates.
pub fn enforce_rollout(
    metrics: &RolloutMetrics,
    flags: &SessionKernelFeatureFlags,
    policy: &RolloutPromotionPolicy,
) -> RolloutEnforcementOutcome {
    match evaluate_promotion_readiness(
        &metrics.divergence,
        metrics.canonical_fallback_count,
        metrics.minor_projection_divergences,
        policy,
    ) {
        PromotionReadiness::Ready => {
            telemetry::metrics::record_rollout_enforcement("promote", 0);
            RolloutEnforcementOutcome {
                decision: RolloutDecision::Promote,
                effective_flags: flags.clone(),
            }
        }
        PromotionReadiness::Blocked(reasons) => {
            telemetry::metrics::record_rollout_enforcement("rollback", reasons.len());
            RolloutEnforcementOutcome {
                effective_flags: flags
                    .clone()
                    .with_runner_binding_mode("handoff")
                    .with_event_log_primary_mode("legacy_messages"),
                decision: RolloutDecision::RollBack(reasons),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{EventLogPrimaryMode, RunnerBindingMode};

    fn clean_divergence() -> ShadowDivergenceReport {
        ShadowDivergenceReport {
            materialized_messages: 5,
            legacy_messages: 5,
            mismatched_roles: 0,
            materialized_tool_calls: 2,
            baseline_tool_calls: 2,
            mismatched_tool_io: 0,
        }
    }

    /// A canonical/event-primary configuration: kernel on, canonical binding, event-primary.
    fn promoted_flags() -> SessionKernelFeatureFlags {
        let mut flags = SessionKernelFeatureFlags::default();
        flags.session_kernel_enabled = true;
        flags
            .with_runner_binding_mode("canonical")
            .with_event_log_primary_mode("new_sessions")
    }

    #[test]
    fn healthy_metrics_promote_and_leave_flags_unchanged() {
        let flags = promoted_flags();
        let metrics = RolloutMetrics {
            divergence: clean_divergence(),
            ..RolloutMetrics::clean()
        };
        let outcome = enforce_rollout(&metrics, &flags, &RolloutPromotionPolicy::default());
        assert_eq!(outcome.decision, RolloutDecision::Promote);
        assert_eq!(outcome.effective_flags, flags, "promote must not alter the flags");
    }

    #[test]
    fn canonical_divergence_rolls_back_to_conservative_flags() {
        // § Shadow divergence: zero tolerance for canonical loss — any tool-IO mismatch
        // forces rollback regardless of budget.
        let mut divergence = clean_divergence();
        divergence.mismatched_tool_io = 1;
        let metrics = RolloutMetrics {
            divergence,
            ..RolloutMetrics::clean()
        };
        let outcome = enforce_rollout(&metrics, &promoted_flags(), &RolloutPromotionPolicy::default());

        assert!(outcome.decision.is_rollback());
        // The automatic rollback: effective flags revert to the conservative defaults even
        // though the configured flags requested canonical/event-primary.
        assert_eq!(
            outcome.effective_flags.runner_binding_mode(),
            RunnerBindingMode::Handoff
        );
        assert_eq!(
            outcome.effective_flags.event_log_primary_mode(),
            EventLogPrimaryMode::LegacyMessages
        );
    }

    #[test]
    fn any_canonical_fallback_rolls_back() {
        // § Fallback budget: any fallback in a canonical/kernel-primary session blocks
        // promotion to default (default budget 0).
        let metrics = RolloutMetrics {
            divergence: clean_divergence(),
            canonical_fallback_count: 1,
            minor_projection_divergences: 0,
        };
        let outcome = enforce_rollout(&metrics, &promoted_flags(), &RolloutPromotionPolicy::default());
        assert!(outcome.decision.is_rollback());
        assert!(!outcome.effective_flags.use_canonical_runner_binding());
    }
}
