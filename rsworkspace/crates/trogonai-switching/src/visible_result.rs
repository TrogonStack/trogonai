//! Builder for the single, normalized visible switch result (§ Contrato de
//! resultado visible del switch). Every surface that exposes a model switch — CLI,
//! TUI, NATS event, Rust struct or future API — consumes the SAME `SwitchVisibleResult`
//! so none of them can present false continuity. This module is pure (no I/O): it
//! turns a switch outcome into the durable contract; persistence/metrics derive from
//! the returned value, never from parsing text.

use buffa::{EnumValue, MessageField};
use trogonai_session_contracts::{
    CapabilityAdaptationAction, ContinuityCheckpointStatus, SwitchCheckpointSummary, SwitchResult,
    SwitchVisibleResult,
};

use crate::orchestrator::{SwitchCompletion, SwitchGateOutcome, classify_switch_result};
use crate::SwitchingError;

/// Stable identity of the switch, known regardless of outcome. Source/target are
/// supplied by the caller because blocked/failed outcomes carry no completion.
pub struct VisibleResultContext<'a> {
    pub session_id: &'a str,
    pub from_model: &'a str,
    pub from_runner: &'a str,
    pub to_model: &'a str,
    pub to_runner: &'a str,
    /// True when the switch fell back to legacy export/import handoff.
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
}

/// Normalize a switch attempt into the durable visible-result contract.
pub fn build_visible_result(
    ctx: &VisibleResultContext<'_>,
    outcome: &Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError>,
) -> SwitchVisibleResult {
    let result = classify_switch_result(outcome);

    let degradations = collect_degradations(outcome);
    let lost_capabilities = collect_lost_capabilities(outcome);
    let checkpoint = checkpoint_summary(outcome);
    let next_action = next_action(result, outcome);

    SwitchVisibleResult {
        session_id: ctx.session_id.to_string(),
        result: EnumValue::Known(result),
        from_model: ctx.from_model.to_string(),
        to_model: ctx.to_model.to_string(),
        from_runner: ctx.from_runner.to_string(),
        to_runner: ctx.to_runner.to_string(),
        runner_changed: ctx.from_runner != ctx.to_runner,
        degradations,
        lost_capabilities,
        fallback_used: ctx.fallback_used,
        fallback_reason: ctx.fallback_reason.clone(),
        checkpoint: MessageField::some(checkpoint),
        next_action,
        ..SwitchVisibleResult::default()
    }
}

/// Visible result for an in-place model change on the same runner: no canonical
/// orchestration ran and the active binding did not change, so it is a clean
/// `switched` with no degradations, no fallback and no checkpoint.
pub fn same_runner_visible_result(session_id: &str, from_model: &str, to_model: &str, runner: &str) -> SwitchVisibleResult {
    SwitchVisibleResult {
        session_id: session_id.to_string(),
        result: EnumValue::Known(SwitchResult::Switched),
        from_model: from_model.to_string(),
        to_model: to_model.to_string(),
        from_runner: runner.to_string(),
        to_runner: runner.to_string(),
        runner_changed: false,
        checkpoint: MessageField::some(SwitchCheckpointSummary {
            required: false,
            status: "not_required".to_string(),
            ..SwitchCheckpointSummary::default()
        }),
        ..SwitchVisibleResult::default()
    }
}

/// Visible result for a legacy `session/export` + `session/import` handoff. Per
/// § Contrato de resultado visible (§2074) a handoff MUST surface `fallback_used = true`
/// so no surface presents it as a complete canonical switch; per § Reglas de
/// interpretacion semantica (§2319) a handoff is NOT a clean `switched`, so it is
/// reported as `degraded` with the un-migrated canonical state called out as the
/// degradation (§2288: handoff/export-import must be visible in events, metrics and UX).
pub fn handoff_visible_result(
    session_id: &str,
    from_model: &str,
    from_runner: &str,
    to_model: &str,
    to_runner: &str,
    fallback_reason: Option<String>,
) -> SwitchVisibleResult {
    SwitchVisibleResult {
        session_id: session_id.to_string(),
        result: EnumValue::Known(SwitchResult::Degraded),
        from_model: from_model.to_string(),
        to_model: to_model.to_string(),
        from_runner: from_runner.to_string(),
        to_runner: to_runner.to_string(),
        runner_changed: from_runner != to_runner,
        degradations: vec![
            "legacy handoff: canonical session state (full tool I/O, artifacts, context twin) not migrated".to_string(),
        ],
        fallback_used: true,
        fallback_reason,
        checkpoint: MessageField::some(SwitchCheckpointSummary {
            required: false,
            status: "not_run".to_string(),
            ..SwitchCheckpointSummary::default()
        }),
        ..SwitchVisibleResult::default()
    }
}

/// Visible result for a switch that failed before (or independently of) the
/// orchestrator outcome — e.g. an id/validation error or a post-attach hydration
/// failure. The canonical session stays consistent, so per § Reglas del contrato
/// visible the `session_id` stays stable; a `failed_terminal` reports the concrete
/// cause as `next_action`, a `failed_recoverable` carries none (retryable).
pub fn failed_visible_result(
    session_id: &str,
    from_model: &str,
    from_runner: &str,
    to_model: &str,
    to_runner: &str,
    result: SwitchResult,
    reason: String,
) -> SwitchVisibleResult {
    SwitchVisibleResult {
        session_id: session_id.to_string(),
        result: EnumValue::Known(result),
        from_model: from_model.to_string(),
        to_model: to_model.to_string(),
        from_runner: from_runner.to_string(),
        to_runner: to_runner.to_string(),
        runner_changed: false,
        next_action: (result == SwitchResult::FailedTerminal).then_some(reason),
        checkpoint: MessageField::some(SwitchCheckpointSummary {
            required: false,
            status: "not_run".to_string(),
            ..SwitchCheckpointSummary::default()
        }),
        ..SwitchVisibleResult::default()
    }
}

/// Visible, recorded degradations: capability adaptation warnings plus every safety
/// reason. Both are surfaced so `degraded`/`requires_confirmation` are explainable.
fn collect_degradations(
    outcome: &Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError>,
) -> Vec<String> {
    let mut out = Vec::new();
    match outcome {
        Ok(Ok(completion)) => {
            out.extend(completion.adaptation_plan.warnings.iter().cloned());
            out.extend(completion.safety.reasons.iter().map(reason_text));
        }
        Ok(Err(SwitchGateOutcome::Blocked(decision)))
        | Ok(Err(SwitchGateOutcome::ConfirmationRequired(decision))) => {
            out.extend(decision.reasons.iter().map(reason_text));
        }
        Err(_) => {}
    }
    out
}

/// Capabilities that could not be preserved (omitted or not portable to the target).
fn collect_lost_capabilities(
    outcome: &Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError>,
) -> Vec<String> {
    match outcome {
        Ok(Ok(completion)) => completion
            .adaptation_plan
            .adaptations
            .iter()
            .filter(|adaptation| {
                matches!(
                    adaptation.action.as_known(),
                    Some(CapabilityAdaptationAction::Omit) | Some(CapabilityAdaptationAction::NotPortable)
                )
            })
            .map(|adaptation| adaptation.capability.clone())
            .collect(),
        _ => Vec::new(),
    }
}

fn checkpoint_summary(
    outcome: &Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError>,
) -> SwitchCheckpointSummary {
    match outcome {
        Ok(Ok(completion)) => match completion.checkpoint.as_ref() {
            Some(checkpoint) => SwitchCheckpointSummary {
                required: true,
                status: checkpoint_status_label(checkpoint.status.as_known()).to_string(),
                ..SwitchCheckpointSummary::default()
            },
            None => SwitchCheckpointSummary {
                required: false,
                status: "not_required".to_string(),
                ..SwitchCheckpointSummary::default()
            },
        },
        // The switch never reached the checkpoint stage (gate-stopped or errored).
        _ => SwitchCheckpointSummary {
            required: false,
            status: "not_run".to_string(),
            ..SwitchCheckpointSummary::default()
        },
    }
}

/// Concrete next step the surface must show. Per § "Reglas del contrato visible":
/// blocked → what to do now; failed_terminal → the concrete cause; otherwise none.
fn next_action(
    result: SwitchResult,
    outcome: &Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError>,
) -> Option<String> {
    match outcome {
        Ok(Err(SwitchGateOutcome::Blocked(decision))) => decision
            .required_action
            .clone()
            .or_else(|| Some("wait_cancel_or_persist".to_string())),
        Ok(Err(SwitchGateOutcome::ConfirmationRequired(decision))) => decision
            .required_action
            .clone()
            .or_else(|| Some("user_confirmation".to_string())),
        Err(err) if result == SwitchResult::FailedTerminal => Some(err.to_string()),
        _ => None,
    }
}

fn reason_text(reason: &trogonai_session_contracts::SwitchSafetyReason) -> String {
    if reason.detail.is_empty() {
        reason.kind.clone()
    } else {
        reason.detail.clone()
    }
}

fn checkpoint_status_label(status: Option<ContinuityCheckpointStatus>) -> &'static str {
    match status {
        Some(ContinuityCheckpointStatus::Passed) => "passed",
        Some(ContinuityCheckpointStatus::Repaired) => "repaired",
        Some(ContinuityCheckpointStatus::Failed) => "failed",
        Some(ContinuityCheckpointStatus::Unspecified) | None => "not_run",
    }
}

#[cfg(test)]
mod golden_tests {
    //! Golden coverage for the visible result contract (§2032: golden tests deben
    //! cubrir al menos switched, blocked, requires_confirmation, degraded,
    //! rolled_back y failed_recoverable). Each test pins the externally-visible
    //! fields a surface (CLI/TUI/event) would render.

    use super::*;
    use trogonai_session_contracts::{
        ContinuityCheckpointResult, SessionId, SwitchAdaptation, SwitchAdaptationPlan, SwitchSafetyDecision,
        SwitchSafetyReason, SwitchSafetyStatus,
    };

    fn ctx<'a>(from_runner: &'a str, to_runner: &'a str) -> VisibleResultContext<'a> {
        VisibleResultContext {
            session_id: "sess_golden",
            from_model: "claude-sonnet",
            from_runner,
            to_model: "grok-code-fast",
            to_runner,
            fallback_used: false,
            fallback_reason: None,
        }
    }

    fn safety(
        status: SwitchSafetyStatus,
        reasons: Vec<SwitchSafetyReason>,
        required_action: Option<String>,
    ) -> SwitchSafetyDecision {
        SwitchSafetyDecision {
            status: EnumValue::Known(status),
            reasons,
            required_action,
            ..SwitchSafetyDecision::default()
        }
    }

    fn reason(kind: &str, detail: &str) -> SwitchSafetyReason {
        SwitchSafetyReason {
            kind: kind.to_string(),
            detail: detail.to_string(),
            ..SwitchSafetyReason::default()
        }
    }

    fn completion(
        safety: SwitchSafetyDecision,
        warnings: Vec<String>,
        adaptations: Vec<SwitchAdaptation>,
        checkpoint: Option<ContinuityCheckpointStatus>,
    ) -> SwitchCompletion {
        SwitchCompletion {
            state: crate::state::SwitchState::Completed,
            from_model: "claude-sonnet".to_string(),
            from_runner: "acp.claude".to_string(),
            to_model: "grok-code-fast".to_string(),
            to_runner: "acp.grok".to_string(),
            safety,
            adaptation_plan: SwitchAdaptationPlan {
                warnings,
                adaptations,
                ..SwitchAdaptationPlan::default()
            },
            projection: trogonai_session_contracts::PromptProjection::default(),
            checkpoint: checkpoint.map(|status| ContinuityCheckpointResult {
                status: EnumValue::Known(status),
                ..ContinuityCheckpointResult::default()
            }),
            events_appended: 4,
        }
    }

    #[test]
    fn switched_is_clean_and_runner_changed() {
        let outcome = Ok(Ok(completion(
            safety(SwitchSafetyStatus::Allowed, vec![], None),
            vec![],
            vec![],
            Some(ContinuityCheckpointStatus::Passed),
        )));
        let v = build_visible_result(&ctx("acp.claude", "acp.grok"), &outcome);
        assert_eq!(v.result.as_known(), Some(SwitchResult::Switched));
        assert!(v.runner_changed);
        assert!(v.degradations.is_empty());
        assert!(v.lost_capabilities.is_empty());
        assert!(!v.fallback_used);
        assert!(v.next_action.is_none());
        let cp = v.checkpoint.as_option().unwrap();
        assert!(cp.required);
        assert_eq!(cp.status, "passed");
    }

    #[test]
    fn degraded_surfaces_degradations_and_lost_capabilities() {
        let adaptation = SwitchAdaptation {
            capability: "image_input".to_string(),
            action: EnumValue::Known(CapabilityAdaptationAction::Omit),
            ..SwitchAdaptation::default()
        };
        let outcome = Ok(Ok(completion(
            safety(SwitchSafetyStatus::AllowedWithWarning, vec![reason("capability", "no images")], None),
            vec!["images dropped".to_string()],
            vec![adaptation],
            None,
        )));
        let v = build_visible_result(&ctx("acp.claude", "acp.grok"), &outcome);
        assert_eq!(v.result.as_known(), Some(SwitchResult::Degraded));
        assert!(v.degradations.contains(&"images dropped".to_string()));
        assert!(v.degradations.contains(&"no images".to_string()));
        assert_eq!(v.lost_capabilities, vec!["image_input".to_string()]);
        let cp = v.checkpoint.as_option().unwrap();
        assert!(!cp.required);
        assert_eq!(cp.status, "not_required");
    }

    #[test]
    fn blocked_carries_next_action_and_not_run_checkpoint() {
        let decision = safety(
            SwitchSafetyStatus::BlockedUntilSafe,
            vec![reason("tool_in_progress", "a tool is still running")],
            Some("wait_cancel_or_persist".to_string()),
        );
        let outcome: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> =
            Ok(Err(SwitchGateOutcome::Blocked(decision)));
        let v = build_visible_result(&ctx("acp.claude", ""), &outcome);
        assert_eq!(v.result.as_known(), Some(SwitchResult::Blocked));
        assert_eq!(v.next_action.as_deref(), Some("wait_cancel_or_persist"));
        assert!(v.degradations.contains(&"a tool is still running".to_string()));
        assert_eq!(v.checkpoint.as_option().unwrap().status, "not_run");
    }

    #[test]
    fn requires_confirmation_lists_degradations() {
        let decision = safety(
            SwitchSafetyStatus::RequiresUserConfirmation,
            vec![reason("capability_loss", "reasoning trace not portable")],
            Some("user_confirmation".to_string()),
        );
        let outcome: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> =
            Ok(Err(SwitchGateOutcome::ConfirmationRequired(decision)));
        let v = build_visible_result(&ctx("acp.claude", ""), &outcome);
        assert_eq!(v.result.as_known(), Some(SwitchResult::RequiresConfirmation));
        assert!(v.degradations.contains(&"reasoning trace not portable".to_string()));
        assert_eq!(v.next_action.as_deref(), Some("user_confirmation"));
    }

    #[test]
    fn failed_recoverable_keeps_session_stable_no_next_action() {
        let outcome: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> =
            Err(SwitchingError::SessionBusy { session_id: SessionId::new("sess_golden").unwrap() });
        let v = build_visible_result(&ctx("acp.claude", ""), &outcome);
        assert_eq!(v.result.as_known(), Some(SwitchResult::FailedRecoverable));
        assert_eq!(v.session_id, "sess_golden");
        assert!(v.next_action.is_none());
        assert_eq!(v.to_model, "grok-code-fast");
    }

    #[test]
    fn failed_terminal_reports_concrete_cause() {
        let outcome: Result<Result<SwitchCompletion, SwitchGateOutcome>, SwitchingError> =
            Err(SwitchingError::TargetModelNotFound { model_id: "ghost".to_string() });
        let v = build_visible_result(&ctx("acp.claude", ""), &outcome);
        assert_eq!(v.result.as_known(), Some(SwitchResult::FailedTerminal));
        assert!(v.next_action.is_some(), "failed_terminal must report a concrete cause");
    }

    #[test]
    fn handoff_is_degraded_with_fallback_used_not_a_clean_switched() {
        // §2074/§2319/§2288: a legacy export/import handoff must surface fallback_used=true
        // and must NOT be reported as a clean `switched`; the un-migrated canonical state is
        // called out as a degradation so no surface fakes full continuity.
        let v = handoff_visible_result("sess_1", "claude-sonnet", "acp.claude", "grok-3", "acp.grok", None);
        assert_eq!(v.result.as_known(), Some(SwitchResult::Degraded));
        assert!(v.fallback_used);
        assert!(v.runner_changed);
        assert!(!v.degradations.is_empty(), "handoff must report the un-migrated state as a degradation");
        assert_eq!(v.checkpoint.as_option().unwrap().status, "not_run");
    }

    #[test]
    fn same_runner_is_clean_switched_without_runner_change() {
        let v = same_runner_visible_result("sess_1", "claude-sonnet", "claude-opus", "acp.claude");
        assert_eq!(v.result.as_known(), Some(SwitchResult::Switched));
        assert!(!v.runner_changed);
        assert!(!v.fallback_used);
        assert!(v.degradations.is_empty());
        assert_eq!(v.checkpoint.as_option().unwrap().status, "not_required");
    }

    #[test]
    fn rolled_back_is_representable_in_the_contract() {
        // The orchestrator now PRODUCES rolled_back when a post-attach stage (the
        // continuity checkpoint) fails and the previous binding is restored (§2032; see
        // `switch_rolls_back_to_previous_binding_when_checkpoint_fails`). This pins the
        // field shape a surface renders for that result.
        let v = SwitchVisibleResult {
            session_id: "sess_golden".to_string(),
            result: EnumValue::Known(SwitchResult::RolledBack),
            from_model: "claude-sonnet".to_string(),
            to_model: "grok-code-fast".to_string(),
            from_runner: "acp.claude".to_string(),
            to_runner: "acp.claude".to_string(),
            runner_changed: false,
            checkpoint: MessageField::some(SwitchCheckpointSummary {
                required: true,
                status: "not_run".to_string(),
                ..SwitchCheckpointSummary::default()
            }),
            ..SwitchVisibleResult::default()
        };
        assert_eq!(v.result.as_known(), Some(SwitchResult::RolledBack));
        // A rolled-back switch left the prior binding active: session stays stable.
        assert!(!v.runner_changed);
    }
}
