use trogonai_session_contracts::{SessionSnapshotState, SwitchSafetyDecision, SwitchSafetyStatus, ToolCallStatus};

use crate::error::SwitchingError;
use crate::state::ForceSwitchState;

/// Request to force a model switch despite warnings.
#[derive(Debug, Clone)]
pub struct ForceSwitchRequest {
    pub confirmed: bool,
    pub acknowledged_losses: Vec<String>,
}

/// Outcome of evaluating a force switch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForceSwitchOutcome {
    Completed {
        invalidated: Vec<String>,
        pending_reconciliation: Vec<String>,
    },
    Rejected(String),
}

/// Evaluate and apply force switch semantics.
pub fn evaluate_force_switch(
    request: &ForceSwitchRequest,
    session: &SessionSnapshotState,
    safety: &SwitchSafetyDecision,
) -> Result<(ForceSwitchOutcome, ForceSwitchState), SwitchingError> {
    let mut state = ForceSwitchState::NotRequested.transition_to(ForceSwitchState::Requested)?;

    if !request.confirmed {
        let _ = state.transition_to(ForceSwitchState::Rejected)?;
        return Ok((
            ForceSwitchOutcome::Rejected("force switch requires explicit user confirmation".to_string()),
            ForceSwitchState::Rejected,
        ));
    }

    if has_unreconciled_destructive_operation(session) {
        let _ = state.transition_to(ForceSwitchState::Rejected)?;
        return Err(SwitchingError::ForceSwitchRejected {
            detail: "destructive operation not reconciled".to_string(),
        });
    }

    state = state.transition_to(ForceSwitchState::Confirmed)?;

    let mut invalidated = vec![
        "provider_response_ids".to_string(),
        "runner_thread_ids".to_string(),
        "terminal_ids".to_string(),
        "live_processes".to_string(),
    ];
    let mut pending_reconciliation = Vec::new();

    if session.tool_calls.iter().any(|tool| {
        tool.status.as_known() == Some(ToolCallStatus::Started)
            || tool.status.as_known() == Some(ToolCallStatus::RequiresReconciliation)
    }) {
        pending_reconciliation.push("pending_tool_result".to_string());
    }

    if safety.status.as_known() == Some(SwitchSafetyStatus::BlockedUntilSafe) {
        invalidated.push("blocked_safety_conditions_overridden".to_string());
    }

    for loss in &request.acknowledged_losses {
        invalidated.push(format!("acknowledged:{loss}"));
    }

    state = state.transition_to(ForceSwitchState::Completed)?;
    Ok((
        ForceSwitchOutcome::Completed {
            invalidated,
            pending_reconciliation,
        },
        state,
    ))
}

fn has_unreconciled_destructive_operation(session: &SessionSnapshotState) -> bool {
    session.tool_calls.iter().any(|tool| {
        tool.status.as_known() == Some(ToolCallStatus::RequiresReconciliation) && tool.name.contains("delete")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::EnumValue;
    use trogonai_session_contracts::{CanonicalToolCall, SwitchSafetyStatus};

    #[test]
    fn force_switch_rejected_without_confirmation() {
        let session = SessionSnapshotState::default();
        let safety = SwitchSafetyDecision {
            status: EnumValue::Known(SwitchSafetyStatus::RequiresUserConfirmation),
            ..SwitchSafetyDecision::default()
        };
        let (outcome, state) = evaluate_force_switch(
            &ForceSwitchRequest {
                confirmed: false,
                acknowledged_losses: Vec::new(),
            },
            &session,
            &safety,
        )
        .unwrap();
        assert_eq!(
            outcome,
            ForceSwitchOutcome::Rejected("force switch requires explicit user confirmation".to_string())
        );
        assert_eq!(state, ForceSwitchState::Rejected);
    }

    #[test]
    fn force_switch_rejects_unreconciled_destructive_tool() {
        let session = SessionSnapshotState {
            tool_calls: vec![CanonicalToolCall {
                name: "delete_file".to_string(),
                status: EnumValue::Known(ToolCallStatus::RequiresReconciliation),
                ..CanonicalToolCall::default()
            }],
            ..SessionSnapshotState::default()
        };
        let safety = SwitchSafetyDecision::default();
        let err = evaluate_force_switch(
            &ForceSwitchRequest {
                confirmed: true,
                acknowledged_losses: vec!["terminal restart".to_string()],
            },
            &session,
            &safety,
        )
        .unwrap_err();
        assert!(matches!(err, SwitchingError::ForceSwitchRejected { .. }));
    }
}
