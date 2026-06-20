use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_capabilities::{
    ProviderCertificationMatrix, ResolvedCapabilities, SessionCapabilityUsage, detect_session_capability_usage,
};
use trogonai_session_contracts::{
    ArtifactSourceAvailability, CapabilityAdaptationAction, ContinuityCheckpointStatus, SessionSnapshotState,
    SwitchAdaptationPlan, SwitchSafetyDecision, SwitchSafetyReason, SwitchSafetyStatus, ToolCallStatus,
};
use uuid::Uuid;

use crate::config::{DegradationPolicy, SwitchingConfig};
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
    evaluate_external_ref_artifacts(input, &mut reasons, &mut needs_confirmation);
    evaluate_nonportable_runtime(input.session, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_dirty_state(input.session, &mut reasons, &mut needs_confirmation);
    evaluate_context_twin_freshness(input, &mut reasons, &mut needs_confirmation);
    evaluate_adaptation_plan(input, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_indispensable_capabilities(input, &mut reasons, &mut blocked, &mut needs_confirmation);
    evaluate_previous_checkpoint(input.session, &mut reasons, &mut needs_confirmation);
    evaluate_certification(input, &mut reasons, &mut needs_confirmation);
    evaluate_compactor_degradation(input, &mut reasons, &mut needs_confirmation);

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

fn evaluate_artifacts(input: &SwitchSafetyInput<'_>, reasons: &mut Vec<SwitchSafetyReason>, blocked: &mut bool) {
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

/// § Imagen por URL (§985): an image that exists only as a degraded `external_ref` (its
/// bytes were never fetched/stored as canonical truth) must not silently cross a switch —
/// the canonical context is incomplete. The Switch Safety Gate requires explicit
/// confirmation of the degradation before proceeding.
fn evaluate_external_ref_artifacts(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    needs_confirmation: &mut bool,
) {
    for artifact in &input.session.artifacts {
        if artifact.availability.as_known() == Some(ArtifactSourceAvailability::ExternalRef) {
            *needs_confirmation = true;
            reasons.push(reason(
                "image_external_ref_only",
                format!(
                    "image artifact {} exists only as an external reference ({}); its bytes were not preserved canonically",
                    artifact.artifact_id,
                    artifact.source_url.as_deref().unwrap_or("unknown source")
                ),
            ));
        }
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

/// § Terminal and Process Policy (§1161): "si hay dirty state no persistido, bloquear
/// hasta guardar artifact/ref o confirmar degradacion". Uncommitted/dirty files captured
/// in the session's terminal continuity must require explicit confirmation before the
/// switch proceeds, so unpersisted work is never silently carried across a model change.
fn evaluate_dirty_state(
    session: &SessionSnapshotState,
    reasons: &mut Vec<SwitchSafetyReason>,
    needs_confirmation: &mut bool,
) {
    let Some(terminal) = session.terminal.as_option() else {
        return;
    };
    if !terminal.dirty_files.is_empty() {
        *needs_confirmation = true;
        reasons.push(reason(
            "dirty_state_unpersisted",
            format!(
                "{} uncommitted file(s) in the terminal are not persisted: {}",
                terminal.dirty_files.len(),
                terminal.dirty_files.join(", ")
            ),
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

    // § Block vs confirmation matrix — configurable risk policy for a degradation (a
    // portable capability loss). Default `Confirm` preserves the closed rule "degradacion
    // aceptable requiere confirmacion"; `Block` is the stricter risk choice; `Warn` only
    // records it. Non-portable loss still blocks unconditionally in the loop below.
    for warning in &plan.warnings {
        match input.config.degradation_policy() {
            DegradationPolicy::Warn => {}
            DegradationPolicy::Confirm => *needs_confirmation = true,
            DegradationPolicy::Block => *blocked = true,
        }
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
            "target model does not support image input; image references preserved but not sent".to_string(),
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
        && !input
            .certification
            .is_switch_allowed(&from_model, &from_runner, input.target_model, input.target_runner)
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

/// Token Budget Policy: `compactor_model` is an explicit user preference. If the
/// switch would degrade it (the target cannot serve compaction, so it falls back to
/// the session model), the Switch Safety Gate must warn / ask for confirmation so
/// the change to the user's explicit choice is never silent (cambio-modelo.md,
/// "Token Budget, Compaction and Prompt Projection Policy").
fn evaluate_compactor_degradation(
    input: &SwitchSafetyInput<'_>,
    reasons: &mut Vec<SwitchSafetyReason>,
    needs_confirmation: &mut bool,
) {
    let Some(compactor_model) = input
        .session
        .config
        .as_option()
        .and_then(|config| config.compactor_model.clone())
    else {
        // No explicit compactor preference -> the default tracks the session model,
        // nothing to degrade.
        return;
    };

    if !input.target_capabilities.schema.compaction_supported {
        *needs_confirmation = true;
        reasons.push(reason(
            "compactor_model_degraded",
            format!(
                "explicit compactor_model {compactor_model} is unavailable on \
                 {}/{}; compaction will fall back to the session model",
                input.target_runner, input.target_model
            ),
        ));
    }
}

fn has_unreconciled_destructive_state(session: &SessionSnapshotState) -> bool {
    session.tool_calls.iter().any(|tool| {
        tool.status.as_known() == Some(ToolCallStatus::RequiresReconciliation) && tool.name.contains("delete")
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
        ArtifactMetadata, CanonicalToolCall, CapabilitySchema, CapabilitySource, SCHEMA_VERSION_V1, SessionConfig,
        SessionMetadata, SwitchAdaptation,
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
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert_eq!(decision.status.as_known(), Some(SwitchSafetyStatus::BlockedUntilSafe));
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
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, Some(&plan), &config, &certification));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::RequiresUserConfirmation)
        );
    }

    fn degradation_plan() -> SwitchAdaptationPlan {
        SwitchAdaptationPlan {
            warnings: vec!["images will be omitted".to_string()],
            adaptations: vec![SwitchAdaptation {
                capability: "image_input".to_string(),
                action: EnumValue::Known(CapabilityAdaptationAction::UseArtifactRefs),
                ..SwitchAdaptation::default()
            }],
            ..SwitchAdaptationPlan::default()
        }
    }

    fn config_with_policy(policy: DegradationPolicy) -> SwitchingConfig {
        SwitchingConfig::default().with_degradation_policy(policy)
    }

    // A session with NO independent confirmation triggers: fresh Context Twin so the
    // degradation risk policy is the ONLY variable deciding the outcome.
    fn clean_session() -> SessionSnapshotState {
        let mut session = base_session();
        session.context_twin = MessageField::some(trogonai_session_contracts::ContextTwin {
            session_id: "sess_safety".to_string(),
            derived_from_seq: 10,
            ..trogonai_session_contracts::ContextTwin::default()
        });
        session
    }

    // Certify BOTH ends and the switch pair so neither the certification level nor the
    // switch-matrix check independently forces confirmation.
    fn certified_matrix() -> ProviderCertificationMatrix {
        fn entry(
            model: &str,
            runner: &str,
            switch_from: Vec<String>,
            switch_to: Vec<String>,
        ) -> trogonai_capabilities::ProviderCertificationEntry {
            trogonai_capabilities::ProviderCertificationEntry {
                model: model.to_string(),
                runner: runner.to_string(),
                text: true,
                tool_use: true,
                parallel_tools: true,
                image_input: false,
                json_schema: true,
                long_context: true,
                streaming: true,
                artifact_refs: true,
                mcp_tools: true,
                switch_from,
                switch_to,
                certified_level: trogonai_capabilities::CertificationLevel::Production,
                last_verified_at: None,
                probe_results: vec![],
            }
        }
        let mut matrix = ProviderCertificationMatrix::default();
        matrix
            .push_validated(entry(
                "anthropic/claude-sonnet",
                "openrouter",
                Vec::new(),
                vec!["xai/grok-code-fast".to_string()],
            ))
            .expect("valid source entry");
        matrix
            .push_validated(entry(
                "xai/grok-code-fast",
                "xai",
                vec!["anthropic/claude-sonnet".to_string()],
                Vec::new(),
            ))
            .expect("valid target entry");
        matrix
    }

    // § Block vs confirmation matrix: the SAME degradation maps to different outcomes by
    // the configurable risk policy, while never altering the closed irreversible rule.

    #[test]
    fn degradation_policy_confirm_requires_confirmation() {
        let session = clean_session();
        let caps = target_capabilities(true);
        let plan = degradation_plan();
        let config = config_with_policy(DegradationPolicy::Confirm);
        let certification = certified_matrix();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, Some(&plan), &config, &certification));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::RequiresUserConfirmation)
        );
    }

    #[test]
    fn degradation_policy_block_blocks_the_switch() {
        let session = clean_session();
        let caps = target_capabilities(true);
        let plan = degradation_plan();
        let config = config_with_policy(DegradationPolicy::Block);
        let certification = certified_matrix();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, Some(&plan), &config, &certification));
        assert_eq!(decision.status.as_known(), Some(SwitchSafetyStatus::BlockedUntilSafe));
    }

    #[test]
    fn degradation_policy_warn_allows_with_warning() {
        let session = clean_session();
        let caps = target_capabilities(true);
        let plan = degradation_plan();
        let config = config_with_policy(DegradationPolicy::Warn);
        let certification = certified_matrix();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, Some(&plan), &config, &certification));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::AllowedWithWarning)
        );
    }

    #[test]
    fn degradation_policy_default_is_confirm() {
        // The closed rule's default: a degradation requires confirmation (§2229).
        assert_eq!(SwitchingConfig::default().degradation_policy(), DegradationPolicy::Confirm);
    }

    #[test]
    fn external_ref_image_requires_confirmation() {
        // § Imagen por URL (§985): an image that exists only as a degraded external_ref
        // must require explicit confirmation before the switch proceeds.
        let mut session = clean_session();
        session.artifacts.push(ArtifactMetadata {
            artifact_id: "art_img".to_string(),
            availability: EnumValue::Known(ArtifactSourceAvailability::ExternalRef),
            source_url: Some("https://host/img.png".to_string()),
            ..ArtifactMetadata::default()
        });
        let caps = target_capabilities(true);
        let config = SwitchingConfig::default();
        let certification = certified_matrix();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::RequiresUserConfirmation)
        );
    }

    #[test]
    fn stored_image_does_not_trigger_external_ref_confirmation() {
        // A properly stored image (availability = Stored) is canonical truth: no gate.
        let mut session = clean_session();
        session.artifacts.push(ArtifactMetadata {
            artifact_id: "art_img".to_string(),
            availability: EnumValue::Known(ArtifactSourceAvailability::Stored),
            ..ArtifactMetadata::default()
        });
        let caps = target_capabilities(true);
        let config = SwitchingConfig::default();
        let certification = certified_matrix();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert_eq!(decision.status.as_known(), Some(SwitchSafetyStatus::Allowed));
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
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert_eq!(decision.status.as_known(), Some(SwitchSafetyStatus::BlockedUntilSafe));
    }

    #[test]
    fn requires_confirmation_when_explicit_compactor_model_degrades() {
        // User explicitly chose a compactor model; the target cannot serve
        // compaction (compaction_supported defaults to false) -> the gate must warn.
        let mut session = base_session();
        if let Some(config) = session.config.as_option_mut() {
            config.compactor_model = Some("anthropic/claude-haiku".to_string());
        }
        let caps = target_capabilities(true); // tool_use ok, compaction_supported = false
        let config = SwitchingConfig::default();
        let certification = ProviderCertificationMatrix::default();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.kind == "compactor_model_degraded"),
            "explicit compactor_model degradation must be recorded as a gate reason"
        );
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::RequiresUserConfirmation)
        );
    }

    #[test]
    fn dirty_state_requires_confirmation() {
        // § Terminal and Process Policy (§1161): unpersisted dirty files in the captured
        // terminal continuity must require explicit confirmation before the switch.
        let mut session = clean_session();
        session.terminal = MessageField::some(trogonai_session_contracts::TerminalContinuity {
            terminal_cwd: "/repo".to_string(),
            dirty_files: vec!["src/lib.rs".to_string()],
            ..trogonai_session_contracts::TerminalContinuity::default()
        });
        let caps = target_capabilities(true);
        let config = SwitchingConfig::default();
        let certification = certified_matrix();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert!(
            decision.reasons.iter().any(|reason| reason.kind == "dirty_state_unpersisted"),
            "dirty terminal state must be recorded as a gate reason"
        );
        assert_eq!(
            decision.status.as_known(),
            Some(SwitchSafetyStatus::RequiresUserConfirmation)
        );
    }

    #[test]
    fn no_compactor_reason_when_no_explicit_preference() {
        // No explicit compactor_model -> the default tracks the session model, so
        // there is nothing to degrade and the gate adds no compactor reason.
        let session = base_session();
        let caps = target_capabilities(true);
        let config = SwitchingConfig::default();
        let certification = ProviderCertificationMatrix::default();
        let decision = evaluate_switch_safety(&safety_input(&session, &caps, None, &config, &certification));
        assert!(
            !decision
                .reasons
                .iter()
                .any(|reason| reason.kind == "compactor_model_degraded"),
            "no compactor reason without an explicit preference"
        );
    }
}
