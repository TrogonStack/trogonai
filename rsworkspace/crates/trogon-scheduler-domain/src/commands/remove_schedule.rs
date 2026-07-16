use trogon_decider::{Decider, Decision};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::ScheduleId;

#[derive(Debug, Clone)]
pub struct RemoveSchedule {
    pub id: ScheduleId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoveScheduleError {
    #[error("schedule '{id}' does not exist")]
    ScheduleNotFound { id: ScheduleId },
    #[error("schedule '{id}' was deleted")]
    ScheduleDeleted { id: ScheduleId },
    #[error("state value is missing")]
    MissingStateValue,
    #[error("unknown state value: {value}")]
    UnknownStateValue { value: i32 },
}

impl RemoveSchedule {
    pub fn new(id: ScheduleId) -> Self {
        Self { id }
    }
}

impl Decider for RemoveSchedule {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = RemoveScheduleError;
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
            return Err(RemoveScheduleError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(RemoveScheduleError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(RemoveScheduleError::ScheduleNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Ok(Decision::event(v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleRemoved {
                            schedule_id: command.id.as_str().to_string(),
                        }
                        .into(),
                    ),
                }))
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(RemoveScheduleError::ScheduleDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(RemoveScheduleError::UnknownStateValue { value: 0 }),
        }
    }
}

#[cfg(test)]
mod tests;
