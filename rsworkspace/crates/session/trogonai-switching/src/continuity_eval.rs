//! §1165-1200 Continuity Evals (offline).
//!
//! Realizes the verification of the documented offline eval. The full offline eval is:
//!
//! ```text
//! load session            -> materialize the canonical snapshot (baseline)
//! switch model            -> run the switch orchestrator
//! ask next step           -> continuity checkpoint asks the destination (echo offline)
//! compare answer vs twin  -> checkpoint compares acknowledgement with Context Twin
//! verify artifacts/tools/decisions are preserved
//! verify degraded capabilities are reported
//! ```
//!
//! The first four steps are produced by the switch orchestrator + continuity checkpoint
//! (which run offline in echo mode, no live provider). This module implements the last
//! two verification steps as a pure, deterministic check over the canonical snapshots and
//! the switch outcome, plus it surfaces the checkpoint comparison result. A caller (test,
//! CI eval, or the orchestrator) runs a switch and feeds the before/after snapshots and the
//! completion here to get a structured [`ContinuityEvalReport`].

use trogonai_session_contracts::{CapabilityAdaptationAction, ContinuityCheckpointStatus, SessionSnapshotState};

use crate::orchestrator::SwitchCompletion;

/// Structured result of an offline continuity eval (§1189-1198).
#[derive(Debug, Clone, PartialEq)]
pub struct ContinuityEvalReport {
    /// "ask next step" + "compare answer against Context Twin": the checkpoint outcome.
    /// `None` when the switch did not run a checkpoint.
    pub checkpoint_status: Option<ContinuityCheckpointStatus>,
    /// True when artifacts, tool calls, decisions and `compactor_model` all survived the
    /// switch (§1196 "verify artifacts/tools/decisions are preserved").
    pub continuity_preserved: bool,
    /// Specific canonical state that was lost across the switch (empty when preserved).
    pub lost: Vec<String>,
    /// True when degraded capabilities were reported (§1197 "verify degraded capabilities
    /// are reported") — i.e. the adaptation plan recorded warnings or omitted/non-portable
    /// capabilities. Note: a switch with no degradations reports `false` here legitimately;
    /// callers asserting "degradations must be reported" should only do so for sessions that
    /// actually exercise a lost capability.
    pub degradations_reported: bool,
    /// The reported degradations (adaptation warnings + omitted/non-portable capabilities).
    pub degradations: Vec<String>,
}

impl ContinuityEvalReport {
    /// Whether the eval passes: continuity preserved and (when any capability was dropped)
    /// the degradation was reported, never silent (§ Reglas de interpretacion semantica).
    pub fn passed(&self) -> bool {
        self.continuity_preserved
    }
}

/// Evaluate continuity preservation across a switch (§1189-1198, steps 5-6). `baseline` is
/// the canonical snapshot state before the switch; `post` after the switch; `completion`
/// the orchestrator outcome carrying the adaptation plan and checkpoint result.
pub fn evaluate_continuity(
    baseline: &SessionSnapshotState,
    post: &SessionSnapshotState,
    completion: &SwitchCompletion,
) -> ContinuityEvalReport {
    let mut lost = Vec::new();

    // verify tools preserved (completed tool calls are canonical truth, §2200).
    if post.tool_calls.len() < baseline.tool_calls.len() {
        lost.push(format!(
            "tool_calls: {} -> {}",
            baseline.tool_calls.len(),
            post.tool_calls.len()
        ));
    }
    // verify artifacts preserved (shared by ref/hash across the switch, §1378/§2205).
    if post.artifacts.len() < baseline.artifacts.len() {
        lost.push(format!("artifacts: {} -> {}", baseline.artifacts.len(), post.artifacts.len()));
    }
    // verify decisions preserved (Context Twin decisions are operational continuity, §59).
    let base_decisions = baseline.context_twin.as_option().map(|t| t.decisions.len()).unwrap_or(0);
    let post_decisions = post.context_twin.as_option().map(|t| t.decisions.len()).unwrap_or(0);
    if post_decisions < base_decisions {
        lost.push(format!("context_twin.decisions: {base_decisions} -> {post_decisions}"));
    }
    // verify compactor_model preserved across the switch (§ Token Budget Policy, AC-12).
    let base_compactor = baseline.config.as_option().and_then(|config| config.compactor_model.clone());
    let post_compactor = post.config.as_option().and_then(|config| config.compactor_model.clone());
    if base_compactor.is_some() && base_compactor != post_compactor {
        lost.push("compactor_model".to_string());
    }

    // verify degraded capabilities are reported (never silent): adaptation warnings plus
    // any capability omitted or marked non-portable by the plan.
    let mut degradations = completion.adaptation_plan.warnings.clone();
    for adaptation in &completion.adaptation_plan.adaptations {
        if matches!(
            adaptation.action.as_known(),
            Some(CapabilityAdaptationAction::Omit) | Some(CapabilityAdaptationAction::NotPortable)
        ) {
            degradations.push(adaptation.capability.clone());
        }
    }

    ContinuityEvalReport {
        checkpoint_status: completion.checkpoint.as_ref().and_then(|checkpoint| checkpoint.status.as_known()),
        continuity_preserved: lost.is_empty(),
        lost,
        degradations_reported: !degradations.is_empty(),
        degradations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::{EnumValue, MessageField};
    use trogonai_session_contracts::{
        ArtifactMetadata, CanonicalToolCall, ContextTwin, ContinuityCheckpointResult, PromptProjection, SessionConfig,
        SwitchAdaptation, SwitchAdaptationPlan, SwitchSafetyDecision,
    };

    fn completion(plan: SwitchAdaptationPlan, checkpoint: Option<ContinuityCheckpointStatus>) -> SwitchCompletion {
        SwitchCompletion {
            state: crate::state::SwitchState::Completed,
            from_model: "anthropic/claude-sonnet".to_string(),
            from_runner: "openrouter".to_string(),
            to_model: "xai/grok-code-fast".to_string(),
            to_runner: "xai".to_string(),
            safety: SwitchSafetyDecision::default(),
            adaptation_plan: plan,
            projection: PromptProjection::default(),
            checkpoint: checkpoint.map(|status| ContinuityCheckpointResult {
                status: EnumValue::Known(status),
                ..ContinuityCheckpointResult::default()
            }),
            events_appended: 0,
        }
    }

    fn snapshot(tools: usize, artifacts: usize, decisions: usize, compactor: Option<&str>) -> SessionSnapshotState {
        SessionSnapshotState {
            tool_calls: vec![CanonicalToolCall::default(); tools],
            artifacts: vec![ArtifactMetadata::default(); artifacts],
            context_twin: MessageField::some(ContextTwin {
                decisions: (0..decisions).map(|i| format!("d{i}")).collect(),
                ..ContextTwin::default()
            }),
            config: MessageField::some(SessionConfig {
                compactor_model: compactor.map(str::to_string),
                ..SessionConfig::default()
            }),
            ..SessionSnapshotState::default()
        }
    }

    #[test]
    fn preserved_switch_passes_with_no_loss() {
        let base = snapshot(2, 1, 3, Some("anthropic/claude-haiku"));
        let post = snapshot(2, 1, 3, Some("anthropic/claude-haiku"));
        let report = evaluate_continuity(&base, &post, &completion(SwitchAdaptationPlan::default(), Some(ContinuityCheckpointStatus::Passed)));
        assert!(report.passed());
        assert!(report.lost.is_empty());
        assert_eq!(report.checkpoint_status, Some(ContinuityCheckpointStatus::Passed));
    }

    #[test]
    fn lost_tools_artifacts_decisions_compactor_are_detected() {
        let base = snapshot(3, 2, 4, Some("anthropic/claude-haiku"));
        let post = snapshot(1, 0, 2, Some("other-model"));
        let report = evaluate_continuity(&base, &post, &completion(SwitchAdaptationPlan::default(), None));
        assert!(!report.passed());
        assert!(report.lost.iter().any(|l| l.starts_with("tool_calls")));
        assert!(report.lost.iter().any(|l| l.starts_with("artifacts")));
        assert!(report.lost.iter().any(|l| l.starts_with("context_twin.decisions")));
        assert!(report.lost.contains(&"compactor_model".to_string()));
    }

    #[test]
    fn degraded_capability_is_reported_not_silent() {
        let base = snapshot(1, 1, 1, None);
        let post = snapshot(1, 1, 1, None);
        let plan = SwitchAdaptationPlan {
            warnings: vec!["images dropped".to_string()],
            adaptations: vec![SwitchAdaptation {
                capability: "image_input".to_string(),
                action: EnumValue::Known(CapabilityAdaptationAction::Omit),
                ..SwitchAdaptation::default()
            }],
            ..SwitchAdaptationPlan::default()
        };
        let report = evaluate_continuity(&base, &post, &completion(plan, Some(ContinuityCheckpointStatus::Passed)));
        assert!(report.passed(), "preserved canonical state");
        assert!(report.degradations_reported, "the dropped capability must be reported");
        assert!(report.degradations.contains(&"image_input".to_string()));
        assert!(report.degradations.contains(&"images dropped".to_string()));
    }
}
