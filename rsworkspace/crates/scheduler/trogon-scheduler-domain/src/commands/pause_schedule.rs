use trogon_decider::{Decider, Decision, SnapshotCadence, WritePrecondition};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::ScheduleId;

#[derive(Debug, Clone)]
pub struct PauseSchedule {
    pub id: ScheduleId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PauseScheduleError {
    #[error("schedule '{id}' does not exist")]
    ScheduleNotFound { id: ScheduleId },
    #[error("schedule '{id}' was deleted")]
    ScheduleDeleted { id: ScheduleId },
    #[error("schedule '{id}' is already paused")]
    AlreadyPaused { id: ScheduleId },
    #[error("state value is missing")]
    MissingStateValue,
    #[error("unknown state value: {value}")]
    UnknownStateValue { value: i32 },
}

impl PauseSchedule {
    pub fn new(id: ScheduleId) -> Self {
        Self { id }
    }
}

impl Decider for PauseSchedule {
    type StreamId = ScheduleId;
    type State = super::state::ScheduleState;
    type Event = v1::ScheduleEvent;
    type DecideError = PauseScheduleError;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;
    const SNAPSHOT_CADENCE: SnapshotCadence = crate::constants::COMMAND_SNAPSHOT_CADENCE;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let Some(value) = state.state.as_ref() else {
            return Err(PauseScheduleError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(PauseScheduleError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(PauseScheduleError::ScheduleNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(PauseScheduleError::ScheduleDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(PauseScheduleError::AlreadyPaused { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED => Ok(Decision::event(v1::ScheduleEvent {
                event: Some(
                    v1::SchedulePaused {
                        schedule_id: command.id.to_string(),
                    }
                    .into(),
                ),
            })),
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(PauseScheduleError::UnknownStateValue { value: 0 }),
        }
    }
}

#[cfg(test)]
mod tests;
