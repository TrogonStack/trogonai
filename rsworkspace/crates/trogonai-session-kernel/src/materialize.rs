use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::{
    ArtifactMetadata, AssistantMessageStartedPayload, CanonicalMessage, CanonicalToolCall, ContinuityCheckpointResult,
    RunnerBinding, SCHEMA_VERSION_V1, SessionConfig, SessionCreatedPayload, SessionEvent, SessionMetadata,
    SessionSnapshot, SessionSnapshotState, SessionSummary, SessionUsage, SwitchAdaptationPlan, SwitchSafetyDecision,
    TokenUsage, ToolCallRequestedPayload, ToolCallStatus, session_event_payload::Kind,
};

use crate::error::SessionKernelError;

/// Fold session events into a materialized snapshot.
pub fn materialize_from_events(
    session_id: &str,
    events: &[SessionEvent],
    existing: Option<SessionSnapshot>,
) -> Result<SessionSnapshot, SessionKernelError> {
    let mut last_applied_seq = existing.as_ref().map(|snapshot| snapshot.last_applied_seq).unwrap_or(0);
    let mut state = existing
        .and_then(|snapshot| snapshot.state.into_option())
        .unwrap_or_default();
    let mut seen_event_ids = std::collections::HashSet::new();
    let mut seen_idempotency = std::collections::HashSet::new();

    for event in events {
        if !seen_event_ids.insert(event.event_id.clone()) {
            continue;
        }
        if !seen_idempotency.insert(event.idempotency_key.clone()) {
            continue;
        }
        if event.seq <= last_applied_seq {
            continue;
        }
        apply_event(&mut state, event)?;
        last_applied_seq = event.seq;
    }

    Ok(SessionSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        session_id: session_id.to_string(),
        last_applied_seq,
        state: MessageField::some(state),
        materialized_at: MessageField::some(now_timestamp()),
        ..SessionSnapshot::default()
    })
}

fn apply_event(state: &mut SessionSnapshotState, event: &SessionEvent) -> Result<(), SessionKernelError> {
    let Some(payload) = event.payload.as_option() else {
        return Ok(());
    };
    let Some(kind) = payload.kind.clone() else {
        return Ok(());
    };
    apply_payload(state, event, kind)
}

fn apply_payload(state: &mut SessionSnapshotState, event: &SessionEvent, kind: Kind) -> Result<(), SessionKernelError> {
    // Stamp event-derived timestamps with the EVENT's own time (when it happened),
    // not the materialization time. This preserves timestamps in the canonical
    // snapshot (§11 "timestamps") and makes replay deterministic — re-materializing
    // the same events yields the same snapshot (§1999). Only `materialized_at` (the
    // snapshot envelope) legitimately uses the materialization clock.
    let event_ts = event.created_at.as_option().cloned().unwrap_or_default();
    match kind {
        Kind::SessionCreated(payload) => apply_session_created(state, &payload, event_ts.clone()),
        Kind::UserMessageAdded(payload) => {
            append_message(
                state,
                payload.message.as_option().unwrap_or(&CanonicalMessage::default()),
            );
            Ok(())
        }
        Kind::AssistantMessageStarted(payload) => {
            apply_assistant_started(state, &payload);
            Ok(())
        }
        Kind::AssistantMessageCompleted(payload) => {
            append_message(
                state,
                payload.message.as_option().unwrap_or(&CanonicalMessage::default()),
            );
            if let Some(usage) = payload.usage.as_option() {
                merge_token_usage(state, usage);
            }
            Ok(())
        }
        Kind::ToolCallRequested(payload) => {
            upsert_tool_call(state, tool_call_from_requested(&payload));
            Ok(())
        }
        Kind::ToolCallApproved(payload) => {
            update_tool_call(state, &payload.tool_call_id, |tool| {
                if tool.status == EnumValue::Known(ToolCallStatus::Pending) {
                    tool.status = EnumValue::Known(ToolCallStatus::Started);
                }
            });
            Ok(())
        }
        Kind::ToolCallStarted(payload) => {
            update_tool_call(state, &payload.tool_call_id, |tool| {
                tool.status = EnumValue::Known(ToolCallStatus::Started);
            });
            Ok(())
        }
        Kind::ToolCallCompleted(payload) => {
            update_tool_call(state, &payload.tool_call_id, |tool| {
                tool.status = EnumValue::Known(ToolCallStatus::Completed);
                tool.result = payload.result.clone();
                tool.completed_at = MessageField::some(event_ts.clone());
            });
            Ok(())
        }
        Kind::ToolCallFailed(payload) => {
            update_tool_call(state, &payload.tool_call_id, |tool| {
                tool.status = EnumValue::Known(ToolCallStatus::Failed);
                tool.error = Some(payload.error.clone());
                tool.completed_at = MessageField::some(event_ts.clone());
            });
            Ok(())
        }
        Kind::ArtifactCreated(payload) => {
            if let Some(artifact) = payload.artifact.as_option() {
                upsert_artifact(state, artifact.clone());
            }
            Ok(())
        }
        Kind::FileChanged(_) => Ok(()),
        Kind::SummaryCreated(payload) => {
            state.summaries.push(SessionSummary {
                summary_id: payload.summary_id.clone(),
                content: payload.content.clone(),
                from_seq: payload.from_seq,
                to_seq: payload.to_seq,
                created_at: MessageField::some(event_ts.clone()),
                ..SessionSummary::default()
            });
            Ok(())
        }
        Kind::ContextTwinUpdated(payload) => {
            if let Some(context_twin) = payload.context_twin.into_option() {
                state.context_twin = MessageField::some(context_twin);
            }
            Ok(())
        }
        Kind::SwitchAdaptationPlanCreated(payload) => {
            state.switch_adaptation_plan = MessageField::some(SwitchAdaptationPlan {
                plan_id: payload.plan_id.clone(),
                from_model: payload.from_model.clone(),
                to_model: payload.to_model.clone(),
                adaptations: payload.adaptations.clone(),
                warnings: payload.warnings.clone(),
                created_at: MessageField::some(event_ts.clone()),
                ..SwitchAdaptationPlan::default()
            });
            Ok(())
        }
        Kind::SwitchSafetyEvaluated(payload) => {
            state.switch_safety = MessageField::some(SwitchSafetyDecision {
                evaluation_id: payload.evaluation_id.clone(),
                status: payload.status,
                reasons: payload.reasons.clone(),
                required_action: payload.required_action.clone(),
                evaluated_at: MessageField::some(event_ts.clone()),
                ..SwitchSafetyDecision::default()
            });
            Ok(())
        }
        Kind::ContinuityCheckpointStarted(_) => Ok(()),
        Kind::ContinuityCheckpointCompleted(payload) => {
            state.continuity_checkpoint = MessageField::some(ContinuityCheckpointResult {
                checkpoint_id: payload.checkpoint_id.clone(),
                status: payload.status,
                confidence: payload.confidence,
                mismatches: payload.mismatches.clone(),
                repairs_applied: payload.repairs_applied.clone(),
                completed_at: MessageField::some(event_ts.clone()),
                ..ContinuityCheckpointResult::default()
            });
            Ok(())
        }
        Kind::ModelSwitched(payload) => {
            let config = state.config.get_or_insert_default();
            config.model = Some(payload.to_model.clone());
            config.runner = Some(payload.to_runner.clone());
            Ok(())
        }
        Kind::RunnerAttached(payload) => {
            state.active_runner_binding = MessageField::some(RunnerBinding {
                schema_version: SCHEMA_VERSION_V1,
                session_id: event.session_id.clone(),
                runner_id: payload.runner_id.clone(),
                model_id: payload.model_id.clone(),
                status: EnumValue::Known(trogonai_session_contracts::RunnerBindingStatus::Attached),
                attached_at: MessageField::some(event_ts.clone()),
                capability_snapshot_id: payload.capability_snapshot_id.clone(),
                ..RunnerBinding::default()
            });
            Ok(())
        }
        Kind::RunnerDetached(payload) => {
            if let Some(binding) = state.active_runner_binding.as_option_mut() {
                binding.status = EnumValue::Known(trogonai_session_contracts::RunnerBindingStatus::Detached);
                binding.detached_at = MessageField::some(event_ts.clone());
                binding.detach_reason = payload.reason.clone();
            }
            Ok(())
        }
        Kind::PermissionRuleAdded(payload) => {
            let config = state.config.get_or_insert_default();
            let existing = config.permission_rules_text.clone().unwrap_or_default();
            config.permission_rules_text = Some(format!("{existing}\n{}", payload.rule_text));
            Ok(())
        }
        Kind::TodoUpdated(payload) => {
            state.todos = payload.todos.clone();
            Ok(())
        }
        Kind::SessionCompacted(payload) => {
            state
                .summaries
                .retain(|summary| summary.summary_id != payload.summary_id);
            Ok(())
        }
        Kind::SessionBranched(payload) => {
            let session = state.session.get_or_insert_default();
            session.parent_session_id = Some(payload.parent_session_id.clone());
            session.branched_at_seq = Some(payload.branched_at_seq);
            Ok(())
        }
        // Cancellation/abort flow events are a durable, replayable audit trail; they
        // do not mutate materialized conversation state (§ Cancellation and Abort Semantics).
        Kind::OperationCancelRequested(_)
        | Kind::RunnerCancelRequested(_)
        | Kind::RunnerCancelled(_)
        | Kind::OperationCancelled(_)
        | Kind::OperationCancelFailed(_)
        | Kind::OperationRequiresReconciliation(_) => Ok(()),
        // Force-switch flow events (§ Force Switch and User Override) — audit only.
        Kind::ForceSwitchRequested(_)
        | Kind::ForceSwitchConfirmed(_)
        | Kind::ForceSwitchCompleted(_)
        | Kind::ForceSwitchRejected(_) => Ok(()),
        // Runner failure invalidates the active binding while preserving session
        // state (§ Failure Mode Policy).
        Kind::RunnerFailed(_) => {
            state.active_runner_binding = MessageField::none();
            Ok(())
        }
        // Invalid event rejection is recorded for audit (§ Failure Mode Policy).
        Kind::InvalidEventRejected(_) => Ok(()),
        // Event-log compaction/retention events are audit trail
        // (§ Event Log Compaction and Retention).
        Kind::SnapshotCreated(_) | Kind::EventsArchived(_) | Kind::ArtifactGcMarked(_) | Kind::ArtifactGcDeleted(_) => {
            Ok(())
        }
        // Redaction is recorded for audit (§ Security, Secrets and Sanitized Exports).
        Kind::RedactionApplied(_) => Ok(()),
        // Preserved terminal/process continuity (§ Terminal and Process Policy).
        Kind::TerminalContinuityCaptured(payload) => {
            state.terminal = payload.terminal.clone();
            Ok(())
        }
        // Compactor-model preservation/degradation events are audit trail; the
        // preserved value already lives in SessionConfig (§ Token Budget Policy).
        Kind::CompactorModelPreserved(_) | Kind::CompactorModelUnavailable(_) | Kind::FallbackToDefaultCompactor(_) => {
            Ok(())
        }
        // Durable audit record of the normalized switch outcome (§ Contrato formal de
        // resultado del switch). The event itself is the durable association; the
        // materialized projection of session state is unchanged by it.
        Kind::SwitchOutcomeRecorded(_) => Ok(()),
    }
}

fn apply_session_created(
    state: &mut SessionSnapshotState,
    payload: &SessionCreatedPayload,
    event_ts: Timestamp,
) -> Result<(), SessionKernelError> {
    state.session = MessageField::some(SessionMetadata {
        id: state
            .session
            .as_option()
            .map(|session| session.id.clone())
            .unwrap_or_default(),
        title: payload.title.clone(),
        cwd: payload.cwd.clone(),
        created_at: MessageField::some(event_ts.clone()),
        updated_at: MessageField::some(event_ts),
        ..SessionMetadata::default()
    });
    state.config = MessageField::some(SessionConfig {
        model: payload.model.clone(),
        runner: payload.runner.clone(),
        compactor_model: payload.compactor_model.clone(),
        ..SessionConfig::default()
    });
    Ok(())
}

fn apply_assistant_started(state: &mut SessionSnapshotState, payload: &AssistantMessageStartedPayload) {
    let config = state.config.get_or_insert_default();
    config.model = Some(payload.model.clone());
    config.runner = Some(payload.runner.clone());
}

fn append_message(state: &mut SessionSnapshotState, message: &CanonicalMessage) {
    if message.message_id.is_empty() {
        return;
    }
    if let Some(existing) = state
        .conversation
        .iter_mut()
        .find(|entry| entry.message_id == message.message_id)
    {
        *existing = message.clone();
    } else {
        state.conversation.push(message.clone());
    }
}

fn merge_token_usage(state: &mut SessionSnapshotState, usage: &TokenUsage) {
    merge_usage(
        state,
        &SessionUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            ..SessionUsage::default()
        },
    );
}

fn merge_usage(state: &mut SessionSnapshotState, usage: &SessionUsage) {
    state.usage = MessageField::some(SessionUsage {
        input_tokens: state.usage.as_option().map(|u| u.input_tokens).unwrap_or(0) + usage.input_tokens,
        output_tokens: state.usage.as_option().map(|u| u.output_tokens).unwrap_or(0) + usage.output_tokens,
        cache_creation_tokens: state.usage.as_option().map(|u| u.cache_creation_tokens).unwrap_or(0)
            + usage.cache_creation_tokens,
        cache_read_tokens: state.usage.as_option().map(|u| u.cache_read_tokens).unwrap_or(0) + usage.cache_read_tokens,
        ..SessionUsage::default()
    });
}

fn tool_call_from_requested(payload: &ToolCallRequestedPayload) -> CanonicalToolCall {
    CanonicalToolCall {
        id: payload.tool_call_id.clone(),
        parent_tool_use_id: payload.parent_tool_use_id.clone(),
        name: payload.name.clone(),
        input_json: payload.input_json.clone(),
        status: EnumValue::Known(ToolCallStatus::Pending),
        tool_execution_id: payload.tool_execution_id.clone(),
        started_at: MessageField::some(now_timestamp()),
        ..CanonicalToolCall::default()
    }
}

fn upsert_tool_call(state: &mut SessionSnapshotState, tool_call: CanonicalToolCall) {
    if let Some(existing) = state.tool_calls.iter_mut().find(|entry| entry.id == tool_call.id) {
        *existing = tool_call;
    } else {
        state.tool_calls.push(tool_call);
    }
}

fn update_tool_call(state: &mut SessionSnapshotState, tool_call_id: &str, update: impl FnOnce(&mut CanonicalToolCall)) {
    if let Some(tool) = state.tool_calls.iter_mut().find(|entry| entry.id == tool_call_id) {
        update(tool);
    }
}

fn upsert_artifact(state: &mut SessionSnapshotState, artifact: ArtifactMetadata) {
    if let Some(existing) = state
        .artifacts
        .iter_mut()
        .find(|entry| entry.artifact_id == artifact.artifact_id)
    {
        *existing = artifact;
    } else {
        state.artifacts.push(artifact);
    }
}

fn now_timestamp() -> Timestamp {
    let now = time::OffsetDateTime::now_utc();
    Timestamp {
        seconds: now.unix_timestamp(),
        nanos: now.nanosecond() as i32,
        ..Timestamp::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogonai_session_contracts::{Actor, ActorType, SessionEventPayload};

    fn sample_created_event(session_id: &str, seq: u64) -> SessionEvent {
        SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: format!("evt_{seq}"),
            session_id: session_id.to_string(),
            seq,
            operation_id: "op_create".to_string(),
            correlation_id: "corr_create".to_string(),
            idempotency_key: format!("idem_{seq}"),
            created_at: MessageField::some(now_timestamp()),
            actor: MessageField::some(Actor {
                r#type: EnumValue::Known(ActorType::Kernel),
                id: "session-kernel".to_string(),
                ..Actor::default()
            }),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    SessionCreatedPayload {
                        title: "Test".to_string(),
                        cwd: "/repo".to_string(),
                        model: Some("anthropic/claude-sonnet".to_string()),
                        runner: Some("openrouter".to_string()),
                        compactor_model: Some("xai/grok-code-fast".to_string()),
                        ..SessionCreatedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        }
    }

    #[test]
    fn materialize_preserves_event_timestamps_not_replay_time() {
        // §11 "timestamps" + §1999 determinism: the snapshot must carry the EVENT's
        // timestamp, not the materialization clock.
        let session_id = "sess_ts";
        let fixed = Timestamp {
            seconds: 1000,
            nanos: 0,
            ..Timestamp::default()
        };
        let mut event = sample_created_event(session_id, 1);
        event.created_at = MessageField::some(fixed.clone());

        let snapshot = materialize_from_events(session_id, &[event.clone()], None).unwrap();
        let session = snapshot
            .state
            .as_option()
            .and_then(|state| state.session.as_option())
            .expect("session metadata");
        assert_eq!(
            session.created_at.as_option().map(|ts| ts.seconds),
            Some(1000),
            "session created_at must be the event timestamp, not replay/wall-clock time"
        );

        // Re-materializing the same event yields the same timestamp (deterministic).
        let again = materialize_from_events(session_id, &[event], None).unwrap();
        let session_again = again
            .state
            .as_option()
            .and_then(|state| state.session.as_option())
            .expect("session metadata");
        assert_eq!(
            session.created_at.as_option().map(|ts| ts.seconds),
            session_again.created_at.as_option().map(|ts| ts.seconds),
            "re-materialization must be deterministic for event timestamps"
        );
    }

    #[test]
    fn materialize_applies_events_in_seq_order_and_deduplicates() {
        let session_id = "sess_materialize";
        let mut events = vec![sample_created_event(session_id, 1)];
        events.push(SessionEvent {
            event_id: "evt_dup".to_string(),
            idempotency_key: "idem_1".to_string(),
            seq: 2,
            ..sample_created_event(session_id, 2)
        });

        let snapshot = materialize_from_events(session_id, &events, None).unwrap();
        assert_eq!(snapshot.last_applied_seq, 1);
        let state = snapshot.state.as_option().unwrap();
        assert_eq!(
            state.config.as_option().unwrap().compactor_model.as_deref(),
            Some("xai/grok-code-fast")
        );
    }
}
