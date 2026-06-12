use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::{
    ContextTwin, ContinuityCheckpointResult, ContinuityCheckpointStatus, ContinuityMismatch,
    ContinuityRepair, PromptProjection, SessionSnapshotState, SwitchAdaptationPlan,
    __buffa::oneof::content_block::Kind as BlockKind,
};
use uuid::Uuid;

use crate::config::SwitchingConfig;
use crate::error::SwitchingError;
use crate::state::ContinuityCheckpointState;
use crate::telemetry;

/// Brief acknowledgement returned by the target runner/model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContinuityAcknowledgement {
    pub current_objective: String,
    pub active_plan: String,
    pub relevant_files: Vec<String>,
    pub last_change: String,
    pub recent_tests: Vec<String>,
    pub open_errors: Vec<String>,
    pub next_step: String,
}

/// Compare acknowledgement against Context Twin and produce checkpoint result.
pub fn compare_acknowledgement(
    context_twin: &ContextTwin,
    acknowledgement: &ContinuityAcknowledgement,
    config: &SwitchingConfig,
) -> ContinuityCheckpointResult {
    let mut mismatches = Vec::new();
    compare_field(
        "current_objective",
        &context_twin.current_objective,
        &acknowledgement.current_objective,
        &mut mismatches,
    );
    compare_field(
        "active_plan",
        &context_twin.active_plan,
        &acknowledgement.active_plan,
        &mut mismatches,
    );
    compare_list(
        "relevant_files",
        &context_twin.relevant_files,
        &acknowledgement.relevant_files,
        &mut mismatches,
    );
    compare_list(
        "open_errors",
        &context_twin.open_errors,
        &acknowledgement.open_errors,
        &mut mismatches,
    );
    compare_field(
        "next_step",
        context_twin.next_steps.first().map(String::as_str).unwrap_or(""),
        &acknowledgement.next_step,
        &mut mismatches,
    );

    let confidence = compute_confidence(&mismatches);
    let status = if confidence >= config.checkpoint_min_confidence {
        EnumValue::Known(ContinuityCheckpointStatus::Passed)
    } else if confidence >= 1.0 - config.checkpoint_mismatch_threshold {
        EnumValue::Known(ContinuityCheckpointStatus::Repaired)
    } else {
        EnumValue::Known(ContinuityCheckpointStatus::Failed)
    };

    let repairs_applied = if status.as_known() == Some(ContinuityCheckpointStatus::Repaired) {
        vec![ContinuityRepair {
            kind: "recompile_with_more_context".to_string(),
            detail: "include additional Context Twin blocks in next projection".to_string(),
            ..ContinuityRepair::default()
        }]
    } else {
        Vec::new()
    };

    ContinuityCheckpointResult {
        checkpoint_id: format!("checkpoint_{}", Uuid::now_v7()),
        status,
        confidence,
        mismatches,
        repairs_applied,
        completed_at: MessageField::some(now_timestamp()),
        ..ContinuityCheckpointResult::default()
    }
}

/// Whether a switch should run a continuity checkpoint.
pub fn requires_continuity_checkpoint(
    session: &SessionSnapshotState,
    from_runner: &str,
    to_runner: &str,
    adaptation_plan: Option<&SwitchAdaptationPlan>,
    config: &SwitchingConfig,
) -> bool {
    if !config.continuity_checkpoint_enabled {
        return false;
    }

    if from_runner != to_runner {
        return true;
    }

    if session.conversation.len() >= config.long_session_turn_threshold {
        return true;
    }

    if adaptation_plan.is_some_and(|plan| !plan.warnings.is_empty()) {
        return true;
    }

    if session
        .context_twin
        .as_option()
        .is_some_and(|twin| !twin.open_risks.is_empty() || !twin.relevant_tool_results.is_empty())
    {
        return true;
    }

    false
}

/// Whether risky tool calls must be blocked because the most recent continuity
/// checkpoint failed and has not been repaired/confirmed (§ Failure Mode Policy:
/// "si falla Continuity Checkpoint, no ejecutar tool calls riesgosos hasta reparar,
/// confirmar o degradar"). A `Repaired`/`Passed` checkpoint (or none) does not block.
pub fn risky_tools_blocked(session: &SessionSnapshotState) -> bool {
    session
        .continuity_checkpoint
        .as_option()
        .is_some_and(|checkpoint| {
            checkpoint.status.as_known() == Some(ContinuityCheckpointStatus::Failed)
        })
}

/// Build acknowledgement request content from a compiled projection.
pub fn acknowledgement_prompt_from_projection(projection: &PromptProjection) -> String {
    projection
        .included_blocks
        .iter()
        .flat_map(|block| block.content.iter())
        .filter_map(|block| match block.kind.as_ref()? {
            BlockKind::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Derive acknowledgement from Context Twin for deterministic tests and mock runners.
pub fn acknowledgement_from_context_twin(context_twin: &ContextTwin) -> ContinuityAcknowledgement {
    ContinuityAcknowledgement {
        current_objective: context_twin.current_objective.clone(),
        active_plan: context_twin.active_plan.clone(),
        relevant_files: context_twin.relevant_files.clone(),
        last_change: context_twin
            .decisions
            .last()
            .cloned()
            .unwrap_or_default(),
        recent_tests: context_twin
            .test_executions
            .iter()
            .map(|test| format!("{}: {}", test.name, test.result))
            .collect(),
        open_errors: context_twin.open_errors.clone(),
        next_step: context_twin.next_steps.first().cloned().unwrap_or_default(),
    }
}

/// Run continuity checkpoint against target model acknowledgement.
#[allow(unused_assignments)]
pub async fn run_continuity_checkpoint<R: RunnerAcknowledgement>(
    runner: &R,
    session_id: &str,
    target_model: &str,
    context_twin: &ContextTwin,
    projection: &PromptProjection,
    config: &SwitchingConfig,
) -> Result<(ContinuityCheckpointResult, ContinuityCheckpointState), SwitchingError> {
    let mut checkpoint_state = ContinuityCheckpointState::Started;
    let prompt = acknowledgement_prompt_from_projection(projection);
    let acknowledgement = if config.continuity_checkpoint_internal_echo {
        acknowledgement_from_context_twin(context_twin)
    } else {
        // A failure of the runner call itself is a runner failure (distinct from a
        // low-confidence checkpoint), so map it to a dedicated error variant.
        runner.request_acknowledgement(&prompt).await.map_err(|err| {
            SwitchingError::RunnerAcknowledgementFailed {
                detail: err.to_string(),
            }
        })?
    };
    checkpoint_state = ContinuityCheckpointState::Acknowledged;

    let result = compare_acknowledgement(context_twin, &acknowledgement, config);
    checkpoint_state = ContinuityCheckpointState::Compared;

    checkpoint_state = match result.status.as_known() {
        Some(ContinuityCheckpointStatus::Passed) => ContinuityCheckpointState::Passed,
        Some(ContinuityCheckpointStatus::Repaired) => ContinuityCheckpointState::Repaired,
        Some(ContinuityCheckpointStatus::Failed) => ContinuityCheckpointState::Failed,
        _ => ContinuityCheckpointState::Failed,
    };

    let label = match result.status.as_known() {
        Some(ContinuityCheckpointStatus::Passed) => "passed",
        Some(ContinuityCheckpointStatus::Repaired) => "repaired",
        Some(ContinuityCheckpointStatus::Failed) => "failed",
        _ => "unknown",
    };
    telemetry::metrics::record_checkpoint_result(session_id, "", target_model, label);
    if !result.mismatches.is_empty() {
        telemetry::metrics::record_checkpoint_mismatch(session_id, "", target_model);
    }
    if result.status.as_known() == Some(ContinuityCheckpointStatus::Repaired) {
        telemetry::metrics::record_context_repair(session_id, "", target_model);
    }

    if result.status.as_known() == Some(ContinuityCheckpointStatus::Failed) {
        checkpoint_state = ContinuityCheckpointState::Blocked;
        return Err(SwitchingError::CheckpointFailed {
            detail: format!(
                "confidence {:.2} below threshold {:.2}",
                result.confidence, config.checkpoint_min_confidence
            ),
        });
    }

    Ok((result, checkpoint_state))
}

/// Placeholder acknowledgement runner used when internal echo is enabled.
#[derive(Clone, Default)]
pub struct PassthroughCheckpointRunner;

#[async_trait::async_trait]
impl RunnerAcknowledgement for PassthroughCheckpointRunner {
    async fn request_acknowledgement(
        &self,
        _prompt: &str,
    ) -> Result<ContinuityAcknowledgement, SwitchingError> {
        Ok(ContinuityAcknowledgement::default())
    }
}

/// Target runner/model acknowledgement interface.
#[async_trait::async_trait]
pub trait RunnerAcknowledgement: Send + Sync {
    async fn request_acknowledgement(
        &self,
        prompt: &str,
    ) -> Result<ContinuityAcknowledgement, SwitchingError>;
}

fn compare_field(field: &str, expected: &str, actual: &str, mismatches: &mut Vec<ContinuityMismatch>) {
    if expected.trim().eq_ignore_ascii_case(actual.trim()) {
        return;
    }
    if expected.is_empty() && actual.is_empty() {
        return;
    }
    mismatches.push(ContinuityMismatch {
        field: field.to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
        ..ContinuityMismatch::default()
    });
}

fn compare_list(
    field: &str,
    expected: &[String],
    actual: &[String],
    mismatches: &mut Vec<ContinuityMismatch>,
) {
    if expected.is_empty() && actual.is_empty() {
        return;
    }
    let overlap = expected
        .iter()
        .filter(|item| actual.iter().any(|actual_item| actual_item == *item))
        .count();
    if overlap == 0 && (!expected.is_empty() || !actual.is_empty()) {
        mismatches.push(ContinuityMismatch {
            field: field.to_string(),
            expected: expected.join(", "),
            actual: actual.join(", "),
            ..ContinuityMismatch::default()
        });
    }
}

fn compute_confidence(mismatches: &[ContinuityMismatch]) -> f64 {
    if mismatches.is_empty() {
        return 1.0;
    }
    let penalty = mismatches.len() as f64 * 0.18;
    (1.0 - penalty).clamp(0.0, 1.0)
}

fn now_timestamp() -> Timestamp {
    Timestamp {
        seconds: time::OffsetDateTime::now_utc().unix_timestamp(),
        nanos: 0,
        ..Timestamp::default()
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;

    /// Mock runner that echoes Context Twin as acknowledgement.
    #[derive(Clone, Default)]
    pub struct MockRunnerAcknowledgement {
        pub context_twin: ContextTwin,
        pub mismatch_objective: bool,
    }

    #[async_trait::async_trait]
    impl RunnerAcknowledgement for MockRunnerAcknowledgement {
        async fn request_acknowledgement(
            &self,
            _prompt: &str,
        ) -> Result<ContinuityAcknowledgement, SwitchingError> {
            let mut ack = acknowledgement_from_context_twin(&self.context_twin);
            if self.mismatch_objective {
                ack.current_objective = "different objective".to_string();
            }
            Ok(ack)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogonai_session_contracts::ContinuityCheckpointResult;

    #[test]
    fn risky_tools_blocked_only_when_checkpoint_failed() {
        // No checkpoint → not blocked.
        let mut state = SessionSnapshotState::default();
        assert!(!risky_tools_blocked(&state));

        // Failed checkpoint → blocked.
        state.continuity_checkpoint = MessageField::some(ContinuityCheckpointResult {
            status: EnumValue::Known(ContinuityCheckpointStatus::Failed),
            ..ContinuityCheckpointResult::default()
        });
        assert!(risky_tools_blocked(&state));

        // Repaired checkpoint → not blocked.
        state.continuity_checkpoint = MessageField::some(ContinuityCheckpointResult {
            status: EnumValue::Known(ContinuityCheckpointStatus::Repaired),
            ..ContinuityCheckpointResult::default()
        });
        assert!(!risky_tools_blocked(&state));
    }

    #[test]
    fn matching_acknowledgement_passes() {
        let twin = ContextTwin {
            schema_version: trogonai_session_contracts::SCHEMA_VERSION_V1,
            session_id: "sess_1".to_string(),
            current_objective: "Fix switch".to_string(),
            active_plan: "Implement safety gate".to_string(),
            relevant_files: vec!["src/safety.rs".to_string()],
            next_steps: vec!["Add tests".to_string()],
            ..ContextTwin::default()
        };
        let ack = acknowledgement_from_context_twin(&twin);
        let result = compare_acknowledgement(&twin, &ack, &SwitchingConfig::default());
        assert_eq!(
            result.status.as_known(),
            Some(ContinuityCheckpointStatus::Passed)
        );
    }

    #[test]
    fn mismatch_fails_checkpoint() {
        let twin = ContextTwin {
            current_objective: "Fix switch".to_string(),
            active_plan: "Implement safety".to_string(),
            relevant_files: vec!["src/lib.rs".to_string()],
            next_steps: vec!["Add tests".to_string()],
            ..ContextTwin::default()
        };
        let ack = ContinuityAcknowledgement {
            current_objective: "Something else".to_string(),
            active_plan: "Different plan".to_string(),
            relevant_files: vec!["other.rs".to_string()],
            next_step: "Unknown".to_string(),
            ..ContinuityAcknowledgement::default()
        };
        let config = SwitchingConfig {
            checkpoint_min_confidence: 0.9,
            ..SwitchingConfig::default()
        };
        let result = compare_acknowledgement(&twin, &ack, &config);
        assert_eq!(
            result.status.as_known(),
            Some(ContinuityCheckpointStatus::Failed)
        );
    }
}
