use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::ScheduleId;

#[derive(Debug, Clone)]
pub struct ResumeSchedule {
    pub id: ScheduleId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResumeScheduleError {
    #[error("schedule '{id}' does not exist")]
    ScheduleNotFound { id: ScheduleId },
    #[error("schedule '{id}' was deleted")]
    ScheduleDeleted { id: ScheduleId },
    #[error("schedule '{id}' is already active")]
    AlreadyActive { id: ScheduleId },
    #[error("schedule '{id}' has already completed its recurrence")]
    AlreadyCompleted { id: ScheduleId },
    #[error("state value is missing")]
    MissingStateValue,
    #[error("unknown state value: {value}")]
    UnknownStateValue { value: i32 },
}

impl ResumeSchedule {
    pub fn new(id: ScheduleId) -> Self {
        Self { id }
    }
}

impl Decider for ResumeSchedule {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = ResumeScheduleError;
    type EvolveError = super::EvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.id.as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &state_v1::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let Some(value) = state.state.as_ref() else {
            return Err(ResumeScheduleError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(ResumeScheduleError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(ResumeScheduleError::ScheduleNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(ResumeScheduleError::ScheduleDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED => {
                Err(ResumeScheduleError::AlreadyActive { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                if state.completed == Some(true) {
                    return Err(ResumeScheduleError::AlreadyCompleted { id: command.id.clone() });
                }
                Ok(Decision::event(v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleResumed {
                            schedule_id: command.id.as_str().to_string(),
                        }
                        .into(),
                    ),
                }))
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(ResumeScheduleError::UnknownStateValue { value: 0 }),
        }
    }
}

impl CommandSnapshotPolicy for ResumeSchedule {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests;
