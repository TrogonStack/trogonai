use buffa::{EnumValue, MessageField};
use trogonai_session_contracts::{
    Actor, CompactorModelPreservedPayload, CompactorModelUnavailablePayload, ContinuityCheckpointCompletedPayload,
    ContinuityCheckpointStartedPayload, FallbackToDefaultCompactorPayload, ForceSwitchCompletedPayload,
    ForceSwitchConfirmedPayload, ForceSwitchRejectedPayload, ForceSwitchRequestedPayload, ModelSwitchReason,
    ModelSwitchedPayload, RunnerAttachedPayload, RunnerDetachedPayload, RunnerFailedPayload, SCHEMA_VERSION_V1,
    SessionEvent, SessionEventPayload, SwitchAdaptationPlanCreatedPayload, SwitchOutcomeRecordedPayload,
    SwitchSafetyEvaluatedPayload, SwitchVisibleResult,
};

use crate::runner::RunnerBindingContext;

pub fn force_switch_requested_event(context: &SwitchFlowContext, acknowledged_losses: &[String]) -> SessionEvent {
    base_event(
        context,
        &format!("evt_force_req_{}", uuid::Uuid::now_v7()),
        "force_switch_requested",
    )
    .tap_payload(ForceSwitchRequestedPayload {
        operation_id: context.operation_id.as_str().to_string(),
        acknowledged_losses: acknowledged_losses.to_vec(),
        ..ForceSwitchRequestedPayload::default()
    })
}

pub fn force_switch_confirmed_event(context: &SwitchFlowContext) -> SessionEvent {
    base_event(
        context,
        &format!("evt_force_ok_{}", uuid::Uuid::now_v7()),
        "force_switch_confirmed",
    )
    .tap_payload(ForceSwitchConfirmedPayload {
        operation_id: context.operation_id.as_str().to_string(),
        ..ForceSwitchConfirmedPayload::default()
    })
}

pub fn force_switch_completed_event(
    context: &SwitchFlowContext,
    invalidated: &[String],
    pending_reconciliation: &[String],
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_force_done_{}", uuid::Uuid::now_v7()),
        "force_switch_completed",
    )
    .tap_payload(ForceSwitchCompletedPayload {
        operation_id: context.operation_id.as_str().to_string(),
        invalidated: invalidated.to_vec(),
        pending_reconciliation: pending_reconciliation.to_vec(),
        ..ForceSwitchCompletedPayload::default()
    })
}

pub fn force_switch_rejected_event(context: &SwitchFlowContext, reason: &str) -> SessionEvent {
    base_event(
        context,
        &format!("evt_force_rej_{}", uuid::Uuid::now_v7()),
        "force_switch_rejected",
    )
    .tap_payload(ForceSwitchRejectedPayload {
        operation_id: context.operation_id.as_str().to_string(),
        reason: reason.to_string(),
        ..ForceSwitchRejectedPayload::default()
    })
}

pub fn runner_failed_event(
    context: &RunnerBindingContext,
    runner_id: &str,
    model_id: &str,
    error: &str,
) -> SessionEvent {
    let flow = switch_flow_from_binding(context);
    base_event(
        &flow,
        &format!("evt_runner_failed_{}", uuid::Uuid::now_v7()),
        "runner_failed",
    )
    .tap_payload(RunnerFailedPayload {
        runner_id: runner_id.to_string(),
        model_id: model_id.to_string(),
        error: error.to_string(),
        ..RunnerFailedPayload::default()
    })
}

pub fn compactor_model_preserved_event(context: &SwitchFlowContext, compactor_model: &str) -> SessionEvent {
    base_event(
        context,
        &format!("evt_compactor_keep_{}", uuid::Uuid::now_v7()),
        "compactor_model_preserved",
    )
    .tap_payload(CompactorModelPreservedPayload {
        compactor_model: compactor_model.to_string(),
        ..CompactorModelPreservedPayload::default()
    })
}

pub fn compactor_model_unavailable_event(
    context: &SwitchFlowContext,
    compactor_model: &str,
    target_model: &str,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_compactor_gone_{}", uuid::Uuid::now_v7()),
        "compactor_model_unavailable",
    )
    .tap_payload(CompactorModelUnavailablePayload {
        compactor_model: compactor_model.to_string(),
        target_model: target_model.to_string(),
        ..CompactorModelUnavailablePayload::default()
    })
}

pub fn fallback_to_default_compactor_event(
    context: &SwitchFlowContext,
    compactor_model: &str,
    target_model: &str,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_compactor_fallback_{}", uuid::Uuid::now_v7()),
        "fallback_to_default_compactor",
    )
    .tap_payload(FallbackToDefaultCompactorPayload {
        compactor_model: compactor_model.to_string(),
        target_model: target_model.to_string(),
        ..FallbackToDefaultCompactorPayload::default()
    })
}

pub fn switch_adaptation_plan_created_event(
    context: &SwitchFlowContext,
    plan: &trogonai_session_contracts::SwitchAdaptationPlan,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_adapt_{}", uuid::Uuid::now_v7()),
        "adaptation_plan",
    )
    .tap_payload(SwitchAdaptationPlanCreatedPayload {
        plan_id: plan.plan_id.clone(),
        from_model: plan.from_model.clone(),
        to_model: plan.to_model.clone(),
        adaptations: plan.adaptations.clone(),
        warnings: plan.warnings.clone(),
        ..SwitchAdaptationPlanCreatedPayload::default()
    })
}

pub fn switch_safety_evaluated_event(
    context: &SwitchFlowContext,
    decision: &trogonai_session_contracts::SwitchSafetyDecision,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_safety_{}", uuid::Uuid::now_v7()),
        "safety_evaluated",
    )
    .tap_payload(SwitchSafetyEvaluatedPayload {
        evaluation_id: decision.evaluation_id.clone(),
        status: decision.status,
        reasons: decision.reasons.clone(),
        required_action: decision.required_action.clone(),
        ..SwitchSafetyEvaluatedPayload::default()
    })
}

pub fn model_switched_event(
    context: &SwitchFlowContext,
    from_runner: &str,
    from_model: &str,
    to_runner: &str,
    to_model: &str,
    reason: ModelSwitchReason,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_switch_{}", uuid::Uuid::now_v7()),
        "model_switched",
    )
    .tap_payload(ModelSwitchedPayload {
        from_runner: from_runner.to_string(),
        from_model: from_model.to_string(),
        to_runner: to_runner.to_string(),
        to_model: to_model.to_string(),
        reason: EnumValue::Known(reason),
        ..ModelSwitchedPayload::default()
    })
}

pub fn runner_attached_event(
    context: &RunnerBindingContext,
    runner_id: &str,
    model_id: &str,
    capability_snapshot_id: Option<String>,
) -> SessionEvent {
    let flow = switch_flow_from_binding(context);
    base_event(
        &flow,
        &format!("evt_attach_{}", uuid::Uuid::now_v7()),
        "runner_attached",
    )
    .tap_payload(RunnerAttachedPayload {
        runner_id: runner_id.to_string(),
        model_id: model_id.to_string(),
        capability_snapshot_id,
        ..RunnerAttachedPayload::default()
    })
}

pub fn runner_detached_event(
    context: &RunnerBindingContext,
    runner_id: &str,
    model_id: &str,
    reason: &str,
) -> SessionEvent {
    let flow = switch_flow_from_binding(context);
    base_event(
        &flow,
        &format!("evt_detach_{}", uuid::Uuid::now_v7()),
        "runner_detached",
    )
    .tap_payload(RunnerDetachedPayload {
        runner_id: runner_id.to_string(),
        model_id: model_id.to_string(),
        reason: Some(reason.to_string()),
        ..RunnerDetachedPayload::default()
    })
}

pub fn continuity_checkpoint_started_event(
    context: &SwitchFlowContext,
    checkpoint_id: &str,
    target_model: &str,
    target_runner: &str,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_ckpt_start_{checkpoint_id}"),
        "checkpoint_started",
    )
    .tap_payload(ContinuityCheckpointStartedPayload {
        checkpoint_id: checkpoint_id.to_string(),
        target_model: target_model.to_string(),
        target_runner: target_runner.to_string(),
        ..ContinuityCheckpointStartedPayload::default()
    })
}

pub fn continuity_checkpoint_completed_event(
    context: &SwitchFlowContext,
    result: &trogonai_session_contracts::ContinuityCheckpointResult,
) -> SessionEvent {
    base_event(
        context,
        &format!("evt_ckpt_done_{}", result.checkpoint_id),
        "checkpoint_completed",
    )
    .tap_payload(ContinuityCheckpointCompletedPayload {
        checkpoint_id: result.checkpoint_id.clone(),
        status: result.status,
        confidence: result.confidence,
        mismatches: result.mismatches.clone(),
        repairs_applied: result.repairs_applied.clone(),
        ..ContinuityCheckpointCompletedPayload::default()
    })
}

/// Shared event metadata for the switch orchestration flow.
#[derive(Clone, Debug)]
pub struct SwitchFlowContext {
    pub session_id: trogonai_session_contracts::SessionId,
    pub operation_id: trogonai_session_contracts::OperationId,
    pub correlation_id: String,
    pub idempotency_key: trogonai_session_contracts::IdempotencyKey,
    pub causation_id: Option<trogonai_session_contracts::EventId>,
    pub actor: Actor,
    pub created_at: buffa_types::google::protobuf::Timestamp,
}

impl SwitchFlowContext {
    pub fn runner_binding_context(&self) -> RunnerBindingContext {
        RunnerBindingContext {
            session_id: self.session_id.clone(),
            operation_id: self.operation_id.clone(),
            correlation_id: self.correlation_id.clone(),
            idempotency_key: self.idempotency_key.clone(),
            causation_id: self.causation_id.clone(),
            actor: self.actor.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

fn switch_flow_from_binding(context: &RunnerBindingContext) -> SwitchFlowContext {
    SwitchFlowContext {
        session_id: context.session_id.clone(),
        operation_id: context.operation_id.clone(),
        correlation_id: context.correlation_id.clone(),
        idempotency_key: context.idempotency_key.clone(),
        causation_id: context.causation_id.clone(),
        actor: context.actor.clone(),
        created_at: context.created_at.clone(),
    }
}

/// Durable audit record of the normalized switch outcome (§ Contrato formal de
/// resultado del switch). Emitted once per switch attempt regardless of result.
pub fn switch_outcome_recorded_event(context: &SwitchFlowContext, result: SwitchVisibleResult) -> SessionEvent {
    base_event(
        context,
        &format!("evt_switch_result_{}", uuid::Uuid::now_v7()),
        "switch_outcome_recorded",
    )
    .tap_payload(SwitchOutcomeRecordedPayload {
        result: MessageField::some(result),
        ..SwitchOutcomeRecordedPayload::default()
    })
}

fn base_event(context: &SwitchFlowContext, event_id: &str, idempotency_suffix: &str) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: event_id.to_string(),
        session_id: context.session_id.as_str().to_string(),
        seq: 0,
        operation_id: context.operation_id.as_str().to_string(),
        correlation_id: context.correlation_id.clone(),
        causation_id: context.causation_id.as_ref().map(|id| id.as_str().to_string()),
        idempotency_key: format!("{}_{idempotency_suffix}", context.idempotency_key.as_str()),
        created_at: MessageField::some(context.created_at.clone()),
        actor: MessageField::some(context.actor.clone()),
        payload: MessageField::some(SessionEventPayload::default()),
        ..SessionEvent::default()
    }
}

trait EventPayloadExt {
    fn tap_payload<P>(self, payload: P) -> SessionEvent
    where
        P: Into<trogonai_session_contracts::session_event_payload::Kind>;
}

impl EventPayloadExt for SessionEvent {
    fn tap_payload<P>(mut self, payload: P) -> SessionEvent
    where
        P: Into<trogonai_session_contracts::session_event_payload::Kind>,
    {
        self.payload = MessageField::some(SessionEventPayload {
            kind: Some(payload.into()),
            ..SessionEventPayload::default()
        });
        self
    }
}
