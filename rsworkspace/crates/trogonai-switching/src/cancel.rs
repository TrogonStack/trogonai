use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::session_event_payload::Kind;
use trogonai_session_contracts::{
    OperationCancelFailedPayload, OperationCancelRequestedPayload, OperationCancelledPayload,
    OperationRequiresReconciliationPayload, RunnerCancelRequestedPayload, RunnerCancelledPayload, SessionEvent,
    SessionEventPayload,
};

use crate::error::SwitchingError;
use crate::state::{CancelState, ToolExecutionState};

/// Context for a cancellation flow.
#[derive(Clone, Debug)]
pub struct CancelContext {
    pub session_id: trogonai_session_contracts::SessionId,
    pub operation_id: trogonai_session_contracts::OperationId,
    pub correlation_id: String,
    pub idempotency_key: trogonai_session_contracts::IdempotencyKey,
    pub actor: trogonai_session_contracts::Actor,
    pub created_at: Timestamp,
    /// Runner being cancelled, when known (empty before a runner is engaged).
    pub runner_id: String,
}

/// Outcome of a cancellation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    Cancelled,
    Failed(String),
    RequiresReconciliation(String),
}

/// Result of asking the runner to cancel an in-flight tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerCancelOutcome {
    /// The runner cancelled the tool before it completed.
    Cancelled,
    /// The tool had already completed, so it could not be cancelled.
    AlreadyCompleted,
}

/// Runner-side cancellation interface.
#[async_trait::async_trait]
pub trait RunnerCancellation: Send + Sync {
    async fn cancel(&self) -> Result<RunnerCancelOutcome, SwitchingError>;
}

/// Execute cancellation semantics: request cancel, wait for runner, emit terminal state.
pub async fn cancel_operation<R: RunnerCancellation>(
    runner: Option<&R>,
    context: &CancelContext,
    tool_states: &[ToolExecutionState],
) -> Result<(CancelOutcome, CancelState, Vec<SessionEvent>), SwitchingError> {
    let mut state = CancelState::Idle.transition_to(CancelState::CancelRequested)?;
    let operation_id = context.operation_id.as_str().to_string();
    let mut events = vec![operation_event(
        context,
        "operation_cancel_requested",
        OperationCancelRequestedPayload {
            operation_id: operation_id.clone(),
            ..OperationCancelRequestedPayload::default()
        }
        .into(),
    )];

    if tool_states.iter().any(|tool| {
        matches!(
            tool,
            ToolExecutionState::Started | ToolExecutionState::RequiresReconciliation
        )
    }) {
        let Some(runner) = runner else {
            state = state.transition_to(CancelState::RequiresReconciliation)?;
            events.push(requires_reconciliation_event(context, "runner unavailable"));
            return Ok((
                CancelOutcome::RequiresReconciliation("runner unavailable".to_string()),
                state,
                events,
            ));
        };

        state = state.transition_to(CancelState::RunnerCancelRequested)?;
        events.push(operation_event(
            context,
            "runner_cancel_requested",
            RunnerCancelRequestedPayload {
                operation_id: operation_id.clone(),
                runner_id: context.runner_id.clone(),
                ..RunnerCancelRequestedPayload::default()
            }
            .into(),
        ));
        match runner.cancel().await {
            Ok(RunnerCancelOutcome::Cancelled) => {
                state = state.transition_to(CancelState::RunnerCancelled)?;
                events.push(operation_event(
                    context,
                    "runner_cancelled",
                    RunnerCancelledPayload {
                        operation_id: operation_id.clone(),
                        runner_id: context.runner_id.clone(),
                        ..RunnerCancelledPayload::default()
                    }
                    .into(),
                ));
            }
            Ok(RunnerCancelOutcome::AlreadyCompleted) => {
                // The tool already finished; the operation cannot be cancelled.
                state = state.transition_to(CancelState::CancelFailed)?;
                let reason = "tool already completed".to_string();
                events.push(operation_event(
                    context,
                    "operation_cancel_failed",
                    OperationCancelFailedPayload {
                        operation_id: operation_id.clone(),
                        error: reason.clone(),
                        ..OperationCancelFailedPayload::default()
                    }
                    .into(),
                ));
                return Ok((CancelOutcome::Failed(reason), state, events));
            }
            Err(err) => {
                state = state.transition_to(CancelState::RequiresReconciliation)?;
                events.push(requires_reconciliation_event(context, &err.to_string()));
                return Ok((CancelOutcome::RequiresReconciliation(err.to_string()), state, events));
            }
        }
    }

    state = state.transition_to(CancelState::OperationCancelled)?;
    events.push(operation_event(
        context,
        "operation_cancelled",
        OperationCancelledPayload {
            operation_id: operation_id.clone(),
            ..OperationCancelledPayload::default()
        }
        .into(),
    ));
    Ok((CancelOutcome::Cancelled, state, events))
}

fn requires_reconciliation_event(context: &CancelContext, reason: &str) -> SessionEvent {
    operation_event(
        context,
        "operation_requires_reconciliation",
        OperationRequiresReconciliationPayload {
            operation_id: context.operation_id.as_str().to_string(),
            reason: reason.to_string(),
            ..OperationRequiresReconciliationPayload::default()
        }
        .into(),
    )
}

fn operation_event(context: &CancelContext, suffix: &str, kind: Kind) -> SessionEvent {
    SessionEvent {
        schema_version: trogonai_session_contracts::SCHEMA_VERSION_V1,
        event_id: format!("evt_{suffix}_{}", uuid::Uuid::now_v7()),
        session_id: context.session_id.as_str().to_string(),
        operation_id: context.operation_id.as_str().to_string(),
        correlation_id: context.correlation_id.clone(),
        idempotency_key: format!("{}_{suffix}", context.idempotency_key.as_str()),
        created_at: MessageField::some(context.created_at.clone()),
        actor: MessageField::some(context.actor.clone()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(kind),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

/// Map canonical tool call statuses into cancellation-relevant execution states.
pub fn tool_states_from_session(
    tool_calls: &[trogonai_session_contracts::CanonicalToolCall],
) -> Vec<ToolExecutionState> {
    tool_calls
        .iter()
        .filter_map(|tool| tool.status.as_known().map(ToolExecutionState::from_tool_call_status))
        .collect()
}

#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;

    #[derive(Default)]
    pub struct MockRunnerCancellation {
        pub should_fail: bool,
        pub already_completed: bool,
    }

    #[async_trait::async_trait]
    impl RunnerCancellation for MockRunnerCancellation {
        async fn cancel(&self) -> Result<RunnerCancelOutcome, SwitchingError> {
            if self.should_fail {
                Err(SwitchingError::CancelFailed {
                    detail: "mock cancel failed".to_string(),
                })
            } else if self.already_completed {
                Ok(RunnerCancelOutcome::AlreadyCompleted)
            } else {
                Ok(RunnerCancelOutcome::Cancelled)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::EnumValue;
    use trogonai_session_contracts::{Actor, ActorType, IdempotencyKey, OperationId};

    fn cancel_context() -> CancelContext {
        CancelContext {
            session_id: trogonai_session_contracts::SessionId::new("sess_cancel").unwrap(),
            operation_id: OperationId::new("op_cancel").unwrap(),
            correlation_id: "corr_cancel".to_string(),
            idempotency_key: IdempotencyKey::new("idem_cancel").unwrap(),
            actor: Actor {
                r#type: EnumValue::Known(ActorType::User),
                id: "user".to_string(),
                ..Actor::default()
            },
            created_at: Timestamp::default(),
            runner_id: "runner_grok".to_string(),
        }
    }

    fn event_kinds(events: &[SessionEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| {
                match event.payload.as_option().and_then(|p| p.kind.as_ref()) {
                    Some(Kind::OperationCancelRequested(_)) => "operation_cancel_requested",
                    Some(Kind::RunnerCancelRequested(_)) => "runner_cancel_requested",
                    Some(Kind::RunnerCancelled(_)) => "runner_cancelled",
                    Some(Kind::OperationCancelled(_)) => "operation_cancelled",
                    Some(Kind::OperationCancelFailed(_)) => "operation_cancel_failed",
                    Some(Kind::OperationRequiresReconciliation(_)) => "operation_requires_reconciliation",
                    _ => "untyped",
                }
                .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn cancel_before_tool_calls_marks_operation_cancelled() {
        let (outcome, state, events) = cancel_operation::<mock::MockRunnerCancellation>(None, &cancel_context(), &[])
            .await
            .unwrap();
        assert_eq!(outcome, CancelOutcome::Cancelled);
        assert_eq!(state, CancelState::OperationCancelled);
        // Cancellation is a typed event flow, not a silent interruption.
        assert_eq!(
            event_kinds(&events),
            vec!["operation_cancel_requested", "operation_cancelled"]
        );
    }

    #[tokio::test]
    async fn cancel_during_tool_emits_full_runner_event_flow() {
        let runner = mock::MockRunnerCancellation {
            should_fail: false,
            ..Default::default()
        };
        let (outcome, state, events) =
            cancel_operation(Some(&runner), &cancel_context(), &[ToolExecutionState::Started])
                .await
                .unwrap();
        assert_eq!(outcome, CancelOutcome::Cancelled);
        assert_eq!(state, CancelState::OperationCancelled);
        assert_eq!(
            event_kinds(&events),
            vec![
                "operation_cancel_requested",
                "runner_cancel_requested",
                "runner_cancelled",
                "operation_cancelled",
            ]
        );
    }

    #[tokio::test]
    async fn cancel_with_runner_failure_requires_reconciliation() {
        let runner = mock::MockRunnerCancellation {
            should_fail: true,
            ..Default::default()
        };
        let (outcome, state, events) =
            cancel_operation(Some(&runner), &cancel_context(), &[ToolExecutionState::Started])
                .await
                .unwrap();
        assert!(matches!(outcome, CancelOutcome::RequiresReconciliation(_)));
        assert_eq!(state, CancelState::RequiresReconciliation);
        assert_eq!(
            event_kinds(&events),
            vec![
                "operation_cancel_requested",
                "runner_cancel_requested",
                "operation_requires_reconciliation",
            ]
        );
    }

    #[tokio::test]
    async fn cancel_when_tool_already_completed_marks_cancel_failed() {
        let runner = mock::MockRunnerCancellation {
            already_completed: true,
            ..Default::default()
        };
        let (outcome, state, events) =
            cancel_operation(Some(&runner), &cancel_context(), &[ToolExecutionState::Started])
                .await
                .unwrap();
        assert!(matches!(outcome, CancelOutcome::Failed(_)));
        assert_eq!(state, CancelState::CancelFailed);
        assert_eq!(
            event_kinds(&events),
            vec![
                "operation_cancel_requested",
                "runner_cancel_requested",
                "operation_cancel_failed",
            ]
        );
    }
}
