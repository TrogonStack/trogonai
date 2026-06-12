use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_capabilities::{ProviderCertificationMatrix, ResolvedCapabilities, SessionCapabilityUsage,
    detect_session_capability_usage,
};
use trogonai_session_contracts::{
    CapabilityAdaptationAction, ContinuityCheckpointStatus, SessionSnapshotState,
    SwitchAdaptationPlan, SwitchSafetyDecision, SwitchSafetyReason, SwitchSafetyStatus,
    ToolCallStatus,
};
use uuid::Uuid;

use crate::config::SwitchingConfig;
use crate::state::{ArtifactPersistenceState, ToolExecutionState};
use crate::telemetry;

/// Inputs for evaluating whether a model switch is safe right now.
#[derive(Debug, Clone)]
pub struct SwitchSafetyInput<'a> {
    pub session: &'a SessionSnapshotState,
    pub target_model: &'a str,
    pub target_runner: &'a str,
    pub target_capabilities: &'a ResolvedCapabilities,
    pub adaptation_plan: Option<&'a SwitchAdaptationPlan>,
    pub certification: &'a ProviderCertificationMatrix,
    pub config: &'a SwitchingConfig,
    pub force: bool,
    pub user_confirmed: bool,
    pub last_applied_seq: u64,
}

/// Evaluate whether it is safe to switch to `target_model` now.
pub fn evaluate_switch_safety(input: &SwitchSafetyInput<'_>) -> SwitchSafetyDecision {
    if !input.config.switch_safety_gate_enabled {
        return allowed_decision("safety_gate_disabled");
    }

    let mut reasons = Vec::new();
    let mut blocked = false;
    let mut needs_confirmation = false;

    evaluate_tool_calls(input.session, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_incomplete_streams(input.session, &mut reasons, &mut blocked);
    evaluate_artifacts(input, &mut reasons, &mut blocked);
    evaluate_nonportable_runtime(input.session, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_context_twin_freshness(input, &mut reasons, &mut needs_confirmation);
    evaluate_adaptation_plan(input, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_indispensable_capabilities(input, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_previous_checkpoint(input.session, &mut reasons, &mut needs_confirmation);
    evaluate_certification(input, &mut reasons, &mut needs_confirmation);

    if input.force {
        if blocked && has_unreconciled_destructive_state(input.session) {
            return blocked_decision(
                "unreconciled_destructive_operation",
                reasons,
                Some("wait_or_reconcile".to_string()),
            );
        }
        needs_confirmation = false;
        blocked = false;
        reasons.push(SwitchSafetyReason {
            kind: "force_switch".to_string(),
            detail: "user explicitly forced switch; known degradation accepted".to_string(),
            ..SwitchSafetyReason::default()
        });
    }

    let status = if blocked {
        EnumValue::Known(SwitchSafetyStatus::BlockedUntilSafe)
    } else if needs_confirmation && !input.user_confirmed {
        EnumValue::Known(SwitchSafetyStatus::RequiresUserConfirmation)
    } else if reasons.is_empty() {
        EnumValue::Known(SwitchSafetyStatus::Allowed)
    } else {
        EnumValue::Known(SwitchSafetyStatus::AllowedWithWarning)
    };

    let required_action = match status.as_known() {
        Some(SwitchSafetyStatus::BlockedUntilSafe) => Some("wait_cancel_or_persist".to_string()),
        Some(SwitchSafetyStatus::RequiresUserConfirmation) => Some("user_confirmation".to_string()),
        _ => None,
    };

    let decision = SwitchSafetyDecision {
        evaluation_id: format!("safety_{}", Uuid::now_v7()),
        status,
        reasons,
        required_action,
        evaluated_at: MessageField::some(now_timestamp()),
        ..SwitchSafetyDecision::default()
    };

    record_safety_metrics(input, &decision);
    decision
}

fn evaluate_tool_calls(
    session: &SessionSnapshotState,
    reasons: &mut Vec<SwitchSafetyReason>,
    blocked: &mut bool,
    needs_confirmation: &mut bool,
) {
    for tool in &session.tool_calls {
        let Some(status) = tool.status.as_known() else {
            continue;
        };
        let state = ToolExecutionState::from_tool_call_status(status);
        if !state.blocks_switch() {
            continue;
        }
        let detail = format!("tool {} ({}) is {:?}", tool.name, tool.id, status);
        match status {
            ToolCallStatus::RequiresReconciliation => {
                *needs_confirmation = true;
                reasons.push(reason("tool_requires_reconciliation", detail));
            }
            ToolCallStatus::Pending | ToolCallStatus::Started => {
                *blocked = true;
                reasons.push(reason("tool_in_progress", detail));
            }
            _ => {}
        }
    }
}

fn evaluate_incomplete_streams(
    session: &SessionSnapshotState,
    reasons: &mut Vec<SwitchSafetyReason>,
    blocked: &mut bool,
) {
    if session
        .nonportable
        .as_option()
        .is_some_and(|state| !state.live_processes.is_empty())
    {
        *blocked = true;
        reasons.push(reason(
            "incomplete_stream",
            "assistant stream or live process still active".to_string(),
        ));
    }
}

fn evaluate_artifacts(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    blocked: &mut bool,
) {
    for artifact in &input.session.artifacts {
        let persistence = artifact_persistence_state(artifact, input.config.inline_artifact_limit_bytes);
        if !persistence.blocks_switch() {
            continue;
        }
        *blocked = true;
        reasons.push(reason(
            "artifact_unpersisted",
            format!(
                "artifact {} is not durably persisted ({:?})",
                artifact.artifact_id, persistence
            ),
        ));
    }
}

fn evaluate_nonportable_runtime(
    session: &SessionSnapshotState,
    reasons: &mut Vec<SwitchSafetyReason>,
    blocked: &mut bool,
    needs_confirmation: &mut bool,
) {
    let Some(nonportable) = session.nonportable.as_option() else {
        return;
    };

    if !nonportable.terminal_ids.is_empty() || !nonportable.live_processes.is_empty() {
        *needs_confirmation = true;
        reasons.push(reason(
            "live_terminal_or_process",
            "terminal or live process state will be restarted on switch".to_string(),
        ));
    }

    if nonportable
        .live_processes
        .iter()
        .any(|process| process.contains("destructive"))
    {
        *blocked = true;
        reasons.push(reason(
            "destructive_operation_pending",
            "destructive operation still running".to_string(),
        ));
    }
}

fn evaluate_context_twin_freshness(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    needs_confirmation: &mut bool,
) {
    let Some(twin) = input.session.context_twin.as_option() else {
        *needs_confirmation = true;
        reasons.push(reason(
            "context_twin_missing",
            "Context Twin is missing; continuity may degrade".to_string(),
        ));
        return;
    };

    if twin.derived_from_seq < input.last_applied_seq {
        *needs_confirmation = true;
        reasons.push(reason(
            "context_twin_stale",
            format!(
                "Context Twin derived_from_seq {} is behind last_applied_seq {}",
                twin.derived_from_seq, input.last_applied_seq
            ),
        ));
    }
}

fn evaluate_adaptation_plan(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    blocked: &mut bool,
    needs_confirmation: &mut bool,
) {
    let Some(plan) = input.adaptation_plan else {
        return;
    };

    for warning in &plan.warnings {
        *needs_confirmation = true;
        reasons.push(reason("capability_degradation", warning.clone()));
    }

    for adaptation in &plan.adaptations {
        if adaptation.action.as_known() == Some(CapabilityAdaptationAction::NotPortable) {
            *blocked = true;
            reasons.push(reason(
                "critical_capability_not_portable",
                format!(
                    "capability {} is not portable to {}",
                    adaptation.capability, input.target_model
                ),
            ));
        }
    }
}

fn evaluate_indispensable_capabilities(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    blocked: &mut bool,
    needs_confirmation: &mut bool,
) {
    let usage = detect_session_capability_usage(input.session);
    if usage.uses_tools && !input.target_capabilities.schema.tool_use {
        if indispensable_tools_in_use(&usage) {
            *blocked = true;
            reasons.push(reason(
                "capability_missing",
                format!(
                    "target model {} does not support tool_use required by session",
                    input.target_model
                ),
            ));
        } else {
            *needs_confirmation = true;
            reasons.push(reason(
                "capability_degradation",
                "tool calls will be textualized for the target model".to_string(),
            ));
        }
    }

    if usage.uses_images && !input.target_capabilities.schema.image_input {
        *needs_confirmation = true;
        reasons.push(reason(
            "capability_degradation",
            "target model does not support image input; image references preserved but not sent"
                .to_string(),
        ));
    }
}

fn evaluate_previous_checkpoint(
    session: &SessionSnapshotState,
    reasons: &mut Vec<SwitchSafetyReason>,
    needs_confirmation: &mut bool,
) {
    let Some(checkpoint) = session.continuity_checkpoint.as_option() else {
        return;
    };
    if checkpoint.status.as_known() == Some(ContinuityCheckpointStatus::Failed) {
        *needs_confirmation = true;
        reasons.push(reason(
            "checkpoint_failed",
            "previous continuity checkpoint failed and has not been repaired".to_string(),
        ));
    }
}

fn evaluate_certification(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    needs_confirmation: &mut bool,
) {
    let from_model = input
        .session
        .config
        .as_option()
        .and_then(|config| config.model.clone())
        .unwrap_or_default();
    let from_runner = input
        .session
        .config
        .as_option()
        .and_then(|config| config.runner.clone())
        .unwrap_or_default();

    let target_level = input
        .certification
        .certification_level(input.target_model, input.target_runner);
    if !target_level.allows_switch_without_warning() {
        *needs_confirmation = true;
        reasons.push(reason(
            "certification_level",
            format!(
                "target {}/{} certified as {:?}",
                input.target_runner, input.target_model, target_level
            ),
        ));
    }

    if !from_model.is_empty()
        && !from_runner.is_empty()
        && !input.certification.is_switch_allowed(
            &from_model,
            &from_runner,
            input.target_model,
            input.target_runner,
        )
    {
        *needs_confirmation = true;
        reasons.push(reason(
            "certification_matrix",
            format!(
                "switch from {from_runner}/{from_model} to {}/{} is not certified",
                input.target_runner, input.target_model
            ),
        ));
    }
}

fn has_unreconciled_destructive_state(session: &SessionSnapshotState) -> bool {
    session.tool_calls.iter().any(|tool| {
        tool.status.as_known() == Some(ToolCallStatus::RequiresReconciliation)
            && tool.name.contains("delete")
    }) || session
        .nonportable
        .as_option()
        .is_some_and(|state| state.live_processes.iter().any(|p| p.contains("destructive")))
}

fn artifact_persistence_state(
    artifact: &trogonai_session_contracts::ArtifactMetadata,
    inline_limit: usize,
) -> ArtifactPersistenceState {
    if artifact.size_bytes <= inline_limit as u64 {
        return ArtifactPersistenceState::Inline;
    }
    if artifact.storage_ref.is_empty() || artifact.sha256.is_empty() {
        ArtifactPersistenceState::PendingStore
    } else {
        ArtifactPersistenceState::Stored
    }
}

fn indispensable_tools_in_use(usage: &SessionCapabilityUsage) -> bool {
    usage.uses_tools && usage.estimated_context_tokens > 0
}

fn allowed_decision(evaluation_id_suffix: &str) -> SwitchSafetyDecision {
    SwitchSafetyDecision {
        evaluation_id: format!("safety_{evaluation_id_suffix}"),
        status: EnumValue::Known(SwitchSafetyStatus::Allowed),
        evaluated_at: MessageField::some(now_timestamp()),
        ..SwitchSafetyDecision::default()
    }
}

fn blocked_decision(
    evaluation_id_suffix: &str,
    reasons: Vec<SwitchSafetyReason>,
    required_action: Option<String>,
) -> SwitchSafetyDecision {
    SwitchSafetyDecision {
        evaluation_id: format!("safety_{evaluation_id_suffix}"),
        status: EnumValue::Known(SwitchSafetyStatus::BlockedUntilSafe),
        reasons,
        required_action,
        evaluated_at: MessageField::some(now_timestamp()),
        ..SwitchSafetyDecision::default()
    }
}

fn reason(kind: &str, detail: String) -> SwitchSafetyReason {
    SwitchSafetyReason {
        kind: kind.to_string(),
        detail,
        ..SwitchSafetyReason::default()
    }
}

fn record_safety_metrics(input: &SwitchSafetyInput<'_>, decision: &SwitchSafetyDecision) {
    let session_id = input
        .session
        .session
        .as_option()
        .map(|session| session.id.as_str())
        .unwrap_or("unknown");

    match decision.status.as_known() {
        Some(SwitchSafetyStatus::BlockedUntilSafe) => {
            let kind = decision
                .reasons
                .first()
                .map(|reason| reason.kind.as_str())
                .unwrap_or("unknown");
            telemetry::metrics::record_switch_blocked(session_id, "", input.target_model, kind);
        }
        Some(SwitchSafetyStatus::RequiresUserConfirmation) => {
            telemetry::metrics::record_switch_confirmation_required(session_id, "", input.target_model);
        }
        _ => {}
    }
}

fn now_timestamp() -> Timestamp {
    Timestamp {
        seconds: time::OffsetDateTime::now_utc().unix_timestamp(),
        nanos: 0,
        ..Timestamp::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::MessageField;
    use trogonai_capabilities::ResolvedCapabilities;
    use trogonai_session_contracts::{
        ArtifactMetadata, CanonicalToolCall, CapabilitySchema, CapabilitySource, SessionConfig,
        SessionMetadata, SwitchAdaptation, SCHEMA_VERSION_V1,
    };

    fn base_session() -> SessionSnapshotState {
        SessionSnapshotState {
            session: MessageField::some(SessionMetadata {
                id: "sess_safety".to_string(),
                title: "Safety".to_string(),
                cwd: "/repo".to_string(),
                ..SessionMetadata::default()
            }),
            config: MessageField::some(SessionConfig {
                model: Some("anthropic/claude-sonnet".to_string()),
                runner: Some("openrouter".to_string()),
                ..SessionConfig::default()
            }),
            ..SessionSnapshotState::default()
        }
    }

    fn target_capabilities(tool_use: bool) -> ResolvedCapabilities {
        ResolvedCapabilities {
            schema: CapabilitySchema {
                schema_version: SCHEMA_VERSION_V1,
                model_id: "xai/grok-code-fast".to_string(),
                runner_id: "xai".to_string(),
                tool_use,
                source: EnumValue::Known(CapabilitySource::Registry),
                ..CapabilitySchema::default()
            },
            runner_id: "xai".to_string(),
            freshness: trogonai_capabilities::FreshnessStatus::Fresh,
            degraded: false,
        }
    }

    fn safety_input<'a>(
        session: &'a SessionSnapshotState,
        target_capabilities: &'a ResolvedCapabilities,
        adaptation_plan: Option<&'a SwitchAdaptationPlan>,
        config: &'a SwitchingConfig,
        certification: &'a ProviderCertificationMatrix,
    ) -> SwitchSafetyInput<'a> {
        SwitchSafetyInput {
            session,
            target_model: "xai/grok-code-fast",
            target_runner: "xai",
            target_capabilities,
            adaptation_plan,
            certification,
            config,
            force: false,
            user_confirmed: false,
            last_applied_seq: 10,
        }
    }

    #[test]
    fn blocks_when_tool_call_in_progress() {
        let mut session = base_session();
        session.tool_calls.push(CanonicalToolCall {
            id: "tool_1".to_string(),
            name: "bash".to_string(),
            status: EnumValue::Known(ToolCallStatus::Started),
            ..CanonicalToolCall::default()
        });
        let caps = target_capabilities(true);
        let config = SwitchingConfig::default();
        let certification = ProviderCertificationMatrix::default();
        let decision = evaluate_switch_safety(&safety_input(
            &session,
            &caps,
            None,
            &config,
            &certification,
        ));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::BlockedUntilSafe)
        );
    }

    #[test]
    fn requires_confirmation_for_capability_degradation() {
        let session = base_session();
        let caps = target_capabilities(true);
        let plan = SwitchAdaptationPlan {
            warnings: vec!["images will be omitted".to_string()],
            adaptations: vec![SwitchAdaptation {
                capability: "image_input".to_string(),
                action: EnumValue::Known(CapabilityAdaptationAction::UseArtifactRefs),
                ..SwitchAdaptation::default()
            }],
            ..SwitchAdaptationPlan::default()
        };
        let config = SwitchingConfig::default();
        let certification = ProviderCertificationMatrix::default();
        let decision = evaluate_switch_safety(&safety_input(
            &session,
            &caps,
            Some(&plan),
            &config,
            &certification,
        ));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::RequiresUserConfirmation)
        );
    }

    #[test]
    fn blocks_unpersisted_large_artifact() {
        let mut session = base_session();
        session.artifacts.push(ArtifactMetadata {
            artifact_id: "art_1".to_string(),
            size_bytes: 200_000,
            ..ArtifactMetadata::default()
        });
        let caps = target_capabilities(true);
        let config = SwitchingConfig::default();
        let certification = ProviderCertificationMatrix::default();
        let decision = evaluate_switch_safety(&safety_input(
            &session,
            &caps,
            None,
            &config,
            &certification,
        ));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::BlockedUntilSafe)
        );
    }
}
