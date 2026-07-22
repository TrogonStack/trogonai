//! Rollout promotion gate: decides whether canonical / event-primary may be promoted
//! to default, per the closed thresholds in cambio-modelo.md ("Valores iniciales
//! cerrados"). Canonical is NOT promoted if the handoff fallback exceeds the budget or
//! if replay/snapshot diverge beyond the threshold; both thresholds must exist before
//! activating default (§ Decisiones finales cerradas, §2240).
//!
//! Pure policy logic: it takes already-measured shadow divergence + fallback counts and
//! returns a decision. Wiring it to an automatic promotion mechanism is operational and
//! out of scope here.

use crate::migration::ShadowDivergenceReport;
use crate::policies::RolloutPromotionPolicy;

/// Whether canonical/event-primary promotion is allowed given measured rollout health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionReadiness {
    Ready,
    Blocked(Vec<PromotionBlockReason>),
}

impl PromotionReadiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, PromotionReadiness::Ready)
    }
}

/// Concrete reason canonical promotion is blocked (§ Shadow divergence / Fallback budget).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionBlockReason {
    /// Canonical loss between materialized replay and authoritative state — zero
    /// tolerance, never permitted under budget (§ Shadow divergence: "cero tolerancia
    /// para perdida canonica, tool IO truncado como verdad, ...").
    CanonicalDivergence {
        mismatched_messages: bool,
        mismatched_roles: usize,
        mismatched_tool_io: usize,
    },
    /// Recorded minor projection-only divergences exceeded the rollout budget. Only
    /// minor projection divergences may be under budget (§ Shadow divergence).
    ProjectionDivergenceOverBudget { observed: usize, budget: usize },
    /// One or more canonical/kernel-primary sessions fell back to handoff
    /// (§ Fallback budget: "cualquier fallback en una sesion kernel-primary/canonical
    /// bloquea promocion a default hasta corregirse").
    CanonicalFallbackOverBudget { observed: usize, budget: usize },
}

/// Evaluate whether canonical/event-primary may be promoted to default.
///
/// - `divergence`: canonical shadow comparison (materialized replay vs legacy truth).
/// - `canonical_fallback_count`: fallbacks to handoff observed in canonical sessions.
/// - `minor_projection_divergences`: recorded minor projection-only divergences.
pub fn evaluate_promotion_readiness(
    divergence: &ShadowDivergenceReport,
    canonical_fallback_count: usize,
    minor_projection_divergences: usize,
    policy: &RolloutPromotionPolicy,
) -> PromotionReadiness {
    let mut reasons = Vec::new();

    // § Shadow divergence: ZERO tolerance for canonical loss. Any message-count, role,
    // or tool-IO mismatch is canonical and is never permitted under budget.
    let mismatched_messages = divergence.materialized_messages != divergence.legacy_messages;
    if mismatched_messages || divergence.mismatched_roles > 0 || divergence.mismatched_tool_io > 0 {
        reasons.push(PromotionBlockReason::CanonicalDivergence {
            mismatched_messages,
            mismatched_roles: divergence.mismatched_roles,
            mismatched_tool_io: divergence.mismatched_tool_io,
        });
    }

    // § Shadow divergence: minor projection divergences allowed only under budget.
    if minor_projection_divergences > policy.max_minor_projection_divergences {
        reasons.push(PromotionBlockReason::ProjectionDivergenceOverBudget {
            observed: minor_projection_divergences,
            budget: policy.max_minor_projection_divergences,
        });
    }

    // § Fallback budget: any canonical fallback over budget (default 0) blocks.
    if canonical_fallback_count > policy.max_canonical_fallbacks {
        reasons.push(PromotionBlockReason::CanonicalFallbackOverBudget {
            observed: canonical_fallback_count,
            budget: policy.max_canonical_fallbacks,
        });
    }

    if reasons.is_empty() {
        PromotionReadiness::Ready
    } else {
        PromotionReadiness::Blocked(reasons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // §2240: both thresholds gate promotion. §2251-2252: zero tolerance for canonical
    // loss + any canonical fallback blocks. Defaults are the strictest (0/0).

    #[test]
    fn clean_shadow_and_no_fallback_is_ready() {
        let policy = RolloutPromotionPolicy::default();
        let readiness = evaluate_promotion_readiness(&clean_divergence(), 0, 0, &policy);
        assert!(readiness.is_ready());
    }

    #[test]
    fn truncated_tool_io_blocks_with_zero_tolerance() {
        let mut divergence = clean_divergence();
        divergence.mismatched_tool_io = 1;
        let readiness = evaluate_promotion_readiness(&divergence, 0, 0, &RolloutPromotionPolicy::default());
        assert!(matches!(
            readiness,
            PromotionReadiness::Blocked(reasons)
                if reasons.iter().any(|r| matches!(r, PromotionBlockReason::CanonicalDivergence { .. }))
        ));
    }

    #[test]
    fn message_count_mismatch_blocks() {
        let mut divergence = clean_divergence();
        divergence.materialized_messages = 4; // lost a message vs legacy 5
        let readiness = evaluate_promotion_readiness(&divergence, 0, 0, &RolloutPromotionPolicy::default());
        assert!(!readiness.is_ready());
    }

    #[test]
    fn any_canonical_fallback_blocks_at_default_budget() {
        let policy = RolloutPromotionPolicy::default();
        assert_eq!(policy.max_canonical_fallbacks, 0);
        let readiness = evaluate_promotion_readiness(&clean_divergence(), 1, 0, &policy);
        assert!(matches!(
            readiness,
            PromotionReadiness::Blocked(reasons)
                if reasons.iter().any(|r| matches!(r, PromotionBlockReason::CanonicalFallbackOverBudget { observed: 1, budget: 0 }))
        ));
    }

    #[test]
    fn minor_projection_divergence_blocks_over_budget_but_not_canonical() {
        // Default budget 0: one minor projection divergence blocks. Canonical clean.
        let strict = RolloutPromotionPolicy::default();
        assert!(!evaluate_promotion_readiness(&clean_divergence(), 0, 1, &strict).is_ready());

        // With an explicit budget, the same minor divergence is allowed.
        let lenient = RolloutPromotionPolicy {
            max_minor_projection_divergences: 2,
            ..RolloutPromotionPolicy::default()
        };
        assert!(evaluate_promotion_readiness(&clean_divergence(), 0, 1, &lenient).is_ready());
    }

    #[test]
    fn canonical_loss_is_never_under_budget() {
        // Even with a generous projection budget, canonical tool-IO loss still blocks.
        let lenient = RolloutPromotionPolicy {
            max_minor_projection_divergences: 100,
            max_canonical_fallbacks: 100,
        };
        let mut divergence = clean_divergence();
        divergence.mismatched_tool_io = 1;
        assert!(!evaluate_promotion_readiness(&divergence, 0, 0, &lenient).is_ready());
    }
}
