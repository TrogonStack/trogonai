//! Explicit state machines for switch, cancel, force-switch, runner binding, tool execution,
//! continuity checkpoint, and artifact persistence flows.

/// Model switch orchestration lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchState {
    Requested,
    LeaseAcquired,
    AdaptationPlanned,
    SafetyEvaluated,
    ModelSwitched,
    RunnerDetached,
    RunnerAttached,
    ProjectionCompiled,
    CheckpointPassed,
    Completed,
    Failed,
    RequiresReconciliation,
}

impl SwitchState {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::Requested, Self::LeaseAcquired)
                | (Self::LeaseAcquired, Self::AdaptationPlanned)
                | (Self::AdaptationPlanned, Self::SafetyEvaluated)
                | (Self::SafetyEvaluated, Self::ModelSwitched)
                | (Self::SafetyEvaluated, Self::Failed)
                | (Self::SafetyEvaluated, Self::RequiresReconciliation)
                | (Self::ModelSwitched, Self::RunnerDetached)
                | (Self::ModelSwitched, Self::RunnerAttached)
                | (Self::RunnerDetached, Self::RunnerAttached)
                | (Self::RunnerAttached, Self::ProjectionCompiled)
                | (Self::ProjectionCompiled, Self::CheckpointPassed)
                | (Self::ProjectionCompiled, Self::Completed)
                | (Self::CheckpointPassed, Self::Completed)
                | (_, Self::Failed)
                | (_, Self::RequiresReconciliation)
        )
    }

    pub fn transition_to(self, next: Self) -> Result<Self, crate::error::SwitchingError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(crate::error::SwitchingError::InvalidSwitchTransition { from: self, to: next })
        }
    }
}

/// Cancellation and abort lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelState {
    Idle,
    CancelRequested,
    RunnerCancelRequested,
    RunnerCancelled,
    OperationCancelled,
    CancelFailed,
    RequiresReconciliation,
}

impl CancelState {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::Idle, Self::CancelRequested)
                | (Self::CancelRequested, Self::RunnerCancelRequested)
                | (Self::CancelRequested, Self::OperationCancelled)
                | (Self::RunnerCancelRequested, Self::RunnerCancelled)
                | (Self::RunnerCancelled, Self::OperationCancelled)
                | (Self::RunnerCancelRequested, Self::RequiresReconciliation)
                | (Self::RunnerCancelled, Self::RequiresReconciliation)
                | (_, Self::CancelFailed)
                | (_, Self::RequiresReconciliation)
        )
    }

    pub fn transition_to(self, next: Self) -> Result<Self, crate::error::SwitchingError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(crate::error::SwitchingError::InvalidCancelTransition { from: self, to: next })
        }
    }
}

/// Force switch override lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForceSwitchState {
    NotRequested,
    Requested,
    Confirmed,
    Completed,
    Rejected,
}

impl ForceSwitchState {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::NotRequested, Self::Requested)
                | (Self::Requested, Self::Confirmed)
                | (Self::Requested, Self::Rejected)
                | (Self::Confirmed, Self::Completed)
        )
    }

    pub fn transition_to(self, next: Self) -> Result<Self, crate::error::SwitchingError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(crate::error::SwitchingError::InvalidForceSwitchTransition { from: self, to: next })
        }
    }
}

/// Runner attach/detach binding lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerBindingState {
    Unbound,
    Attaching,
    Attached,
    Detaching,
    Detached,
    Failed,
}

impl RunnerBindingState {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::Unbound, Self::Attaching)
                | (Self::Attaching, Self::Attached)
                | (Self::Attached, Self::Detaching)
                | (Self::Detaching, Self::Detached)
                | (Self::Detached, Self::Attaching)
                | (_, Self::Failed)
        )
    }
}

/// Tool execution lifecycle aligned with canonical tool call statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionState {
    Pending,
    Approved,
    Started,
    Completed,
    Failed,
    RequiresReconciliation,
}

impl ToolExecutionState {
    pub fn from_tool_call_status(
        status: trogonai_session_contracts::ToolCallStatus,
    ) -> Self {
        use trogonai_session_contracts::ToolCallStatus;
        match status {
            ToolCallStatus::Pending => Self::Pending,
            ToolCallStatus::Started => Self::Started,
            ToolCallStatus::Completed => Self::Completed,
            ToolCallStatus::Failed => Self::Failed,
            ToolCallStatus::RequiresReconciliation => Self::RequiresReconciliation,
            ToolCallStatus::Unspecified => Self::Pending,
        }
    }

    pub fn blocks_switch(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Approved | Self::Started | Self::RequiresReconciliation
        )
    }
}

/// Continuity checkpoint lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuityCheckpointState {
    NotRequired,
    Started,
    Acknowledged,
    Compared,
    Passed,
    Repaired,
    Failed,
    Blocked,
}

impl ContinuityCheckpointState {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (Self::NotRequired, Self::Started)
                | (Self::Started, Self::Acknowledged)
                | (Self::Acknowledged, Self::Compared)
                | (Self::Compared, Self::Passed)
                | (Self::Compared, Self::Repaired)
                | (Self::Compared, Self::Failed)
                | (Self::Compared, Self::Blocked)
                | (Self::Repaired, Self::Passed)
                | (Self::Failed, Self::Blocked)
        )
    }
}

/// Artifact persistence lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactPersistenceState {
    Inline,
    PendingStore,
    Stored,
    Referenced,
    Missing,
    Failed,
}

impl ArtifactPersistenceState {
    pub fn blocks_switch(self) -> bool {
        matches!(self, Self::PendingStore | Self::Missing | Self::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_happy_path_transitions() {
        let mut state = SwitchState::Requested;
        state = state.transition_to(SwitchState::LeaseAcquired).unwrap();
        state = state.transition_to(SwitchState::AdaptationPlanned).unwrap();
        state = state.transition_to(SwitchState::SafetyEvaluated).unwrap();
        state = state.transition_to(SwitchState::ModelSwitched).unwrap();
        state = state.transition_to(SwitchState::RunnerDetached).unwrap();
        state = state.transition_to(SwitchState::RunnerAttached).unwrap();
        state = state.transition_to(SwitchState::ProjectionCompiled).unwrap();
        state = state.transition_to(SwitchState::CheckpointPassed).unwrap();
        state = state.transition_to(SwitchState::Completed).unwrap();
        assert_eq!(state, SwitchState::Completed);
    }

    #[test]
    fn switch_invalid_transition_is_rejected() {
        let err = SwitchState::Requested
            .transition_to(SwitchState::Completed)
            .unwrap_err();
        assert!(matches!(
            err,
            crate::error::SwitchingError::InvalidSwitchTransition { .. }
        ));
    }

    #[test]
    fn cancel_path_supports_reconciliation() {
        let mut state = CancelState::Idle;
        state = state.transition_to(CancelState::CancelRequested).unwrap();
        state = state
            .transition_to(CancelState::RunnerCancelRequested)
            .unwrap();
        state = state.transition_to(CancelState::RequiresReconciliation).unwrap();
        assert_eq!(state, CancelState::RequiresReconciliation);
    }

    #[test]
    fn force_switch_requires_confirmation_before_complete() {
        assert!(ForceSwitchState::NotRequested.can_transition_to(ForceSwitchState::Requested));
        assert!(!ForceSwitchState::Requested.can_transition_to(ForceSwitchState::Completed));
        assert!(ForceSwitchState::Confirmed.can_transition_to(ForceSwitchState::Completed));
    }
}
