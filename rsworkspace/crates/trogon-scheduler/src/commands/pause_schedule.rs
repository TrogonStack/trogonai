use trogon_decider_runtime::{Decider, Decision};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::ScheduleId;

#[derive(Debug, Clone)]
pub struct PauseSchedule {
    pub id: ScheduleId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseScheduleError {
    ScheduleNotFound { id: ScheduleId },
    ScheduleDeleted { id: ScheduleId },
    AlreadyPaused { id: ScheduleId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for PauseScheduleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ScheduleNotFound { id } => write!(formatter, "schedule '{id}' does not exist"),
            Self::ScheduleDeleted { id } => write!(formatter, "schedule '{id}' was deleted"),
            Self::AlreadyPaused { id } => write!(formatter, "schedule '{id}' is already paused"),
            Self::MissingStateValue => formatter.write_str("state value is missing"),
            Self::UnknownStateValue { value } => write!(formatter, "unknown state value: {value}"),
        }
    }
}

impl std::error::Error for PauseScheduleError {}

impl PauseSchedule {
    pub fn new(id: ScheduleId) -> Self {
        Self { id }
    }
}

impl Decider for PauseSchedule {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = PauseScheduleError;
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
                        schedule_id: command.id.as_str().to_string(),
                    }
                    .into(),
                ),
            })),
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(PauseScheduleError::UnknownStateValue { value: 0 })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use trogon_decider::testing::TestCase;

    use super::*;
    use crate::commands::domain::{
        Delivery, MessageContent, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule,
        ScheduleHeaders, ScheduleMessage, ScheduleSpec, ScheduleStatus,
    };

    fn schedule_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn pause_job_command(id: &str) -> PauseSchedule {
        PauseSchedule::new(ScheduleId::parse(id).unwrap())
    }

    fn job(id: &str) -> Schedule {
        Schedule {
            id: schedule_id(id),
            status: ScheduleStatus::Enabled,
            schedule: ScheduleSpec::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: ScheduleMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: ScheduleHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::ScheduleEvent {
        let schedule = job(id);

        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: schedule.id.as_str().to_string(),
                    status: buffa::MessageField::some(v1::ScheduleStatus::from(schedule.status)),
                    schedule: buffa::MessageField::some(v1::Schedule::from(&ScheduleEventSchedule::from(
                        &schedule.schedule,
                    ))),
                    delivery: buffa::MessageField::some(v1::Delivery::from(&ScheduleEventDelivery::from(
                        &schedule.delivery,
                    ))),
                    message: buffa::MessageField::some(v1::Message::from(&MessageEnvelope::from(&schedule.message))),
                }
                .into(),
            ),
        }
    }

    fn paused(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn removed(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    #[test]
    fn given_when_then_supports_pause_job_decider() {
        TestCase::<PauseSchedule>::new()
            .given([added("backup")])
            .when(pause_job_command("backup"))
            .then([paused("backup")]);
    }

    #[test]
    fn given_when_then_supports_pause_job_failures() {
        TestCase::<PauseSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .when(pause_job_command("backup"))
            .then_error(PauseScheduleError::AlreadyPaused {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_pausing_missing_jobs() {
        TestCase::<PauseSchedule>::new()
            .given_no_history()
            .when(pause_job_command("backup"))
            .then_error(PauseScheduleError::ScheduleNotFound {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_pausing_deleted_jobs() {
        TestCase::<PauseSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .given([removed("backup")])
            .when(pause_job_command("backup"))
            .then_error(PauseScheduleError::ScheduleDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn decide_errors_display_user_facing_messages() {
        let id = ScheduleId::parse("backup").unwrap();

        assert_eq!(
            PauseScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
            "schedule 'backup' does not exist"
        );
        assert_eq!(
            PauseScheduleError::ScheduleDeleted { id: id.clone() }.to_string(),
            "schedule 'backup' was deleted"
        );
        assert_eq!(
            PauseScheduleError::AlreadyPaused { id }.to_string(),
            "schedule 'backup' is already paused"
        );
        assert_eq!(PauseScheduleError::MissingStateValue.to_string(), "state value is missing");
        assert_eq!(
            PauseScheduleError::UnknownStateValue { value: 42 }.to_string(),
            "unknown state value: 42"
        );
    }
}
