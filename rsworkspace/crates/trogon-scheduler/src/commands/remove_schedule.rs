use trogon_decider_runtime::{Decider, Decision};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::ScheduleId;

#[derive(Debug, Clone)]
pub struct RemoveSchedule {
    pub id: ScheduleId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveScheduleError {
    ScheduleNotFound { id: ScheduleId },
    ScheduleDeleted { id: ScheduleId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for RemoveScheduleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ScheduleNotFound { id } => write!(formatter, "schedule '{id}' does not exist"),
            Self::ScheduleDeleted { id } => write!(formatter, "schedule '{id}' was deleted"),
            Self::MissingStateValue => formatter.write_str("state value is missing"),
            Self::UnknownStateValue { value } => write!(formatter, "unknown state value: {value}"),
        }
    }
}

impl std::error::Error for RemoveScheduleError {}

fn current_state(state: &state_v1::State) -> Result<state_v1::StateValue, RemoveScheduleError> {
    let Some(value) = state.state.as_ref() else {
        return Err(RemoveScheduleError::MissingStateValue);
    };
    let Some(current_state) = value.as_known() else {
        return Err(RemoveScheduleError::UnknownStateValue { value: value.to_i32() });
    };
    if current_state == state_v1::StateValue::STATE_VALUE_UNSPECIFIED {
        return Err(RemoveScheduleError::UnknownStateValue { value: 0 });
    }
    Ok(current_state)
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
        let current_state = current_state(state)?;
        if current_state == state_v1::StateValue::STATE_VALUE_MISSING {
            return Err(RemoveScheduleError::ScheduleNotFound { id: command.id.clone() });
        }
        if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
            return Err(RemoveScheduleError::ScheduleDeleted { id: command.id.clone() });
        }
        Ok(Decision::event(v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: command.id.as_str().to_string(),
                }
                .into(),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use trogon_decider::testing::TestCase;
    use trogon_decider_runtime::Decider;

    use buffa::{EnumValue, MessageField};

    use super::*;
    use crate::CreateSchedule;
    use crate::commands::domain::{
        Delivery, MessageContent, MessageEnvelope, Schedule as DomainSchedule, ScheduleEventDelivery,
        ScheduleEventSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleId, ScheduleMessage,
    };

    fn create_schedule(id: &str) -> CreateSchedule {
        CreateSchedule {
            id: ScheduleId::parse(id).unwrap(),
            status: ScheduleEventStatus::Scheduled,
            schedule: DomainSchedule::every(std::time::Duration::from_secs(30)).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: ScheduleMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: ScheduleHeaders::default(),
            },
        }
    }

    fn remove_job_command(id: &str) -> RemoveSchedule {
        RemoveSchedule::new(ScheduleId::parse(id).unwrap())
    }

    fn added(id: &str) -> v1::ScheduleEvent {
        let command = create_schedule(id);

        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: command.id.as_str().to_string(),
                    status: MessageField::some(v1::ScheduleStatus::from(command.status)),
                    schedule: MessageField::some(
                        v1::Schedule::try_from(&ScheduleEventSchedule::from(&command.schedule)).unwrap(),
                    ),
                    delivery: MessageField::some(
                        v1::Delivery::try_from(&ScheduleEventDelivery::from(&command.delivery)).unwrap(),
                    ),
                    message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&command.message))),
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

    fn state(value: state_v1::StateValue) -> state_v1::State {
        state_v1::State {
            state: Some(EnumValue::from(value)),
        }
    }

    #[test]
    fn given_when_then_supports_remove_job_decider() {
        TestCase::<RemoveSchedule>::new()
            .given([added("backup")])
            .when(remove_job_command("backup"))
            .then([removed("backup")]);
    }

    #[test]
    fn given_when_then_removes_disabled_jobs() {
        TestCase::<RemoveSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .when(remove_job_command("backup"))
            .then([removed("backup")]);
    }

    #[test]
    fn given_when_then_rejects_removing_missing_jobs() {
        TestCase::<RemoveSchedule>::new()
            .given_no_history()
            .when(remove_job_command("backup"))
            .then_error(RemoveScheduleError::ScheduleNotFound {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_removing_deleted_job() {
        TestCase::<RemoveSchedule>::new()
            .given([added("backup")])
            .given([removed("backup")])
            .when(remove_job_command("backup"))
            .then_error(RemoveScheduleError::ScheduleDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn errors_display_user_facing_messages() {
        let id = ScheduleId::parse("backup").unwrap();

        assert_eq!(
            RemoveScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
            "schedule 'backup' does not exist"
        );
        assert_eq!(
            RemoveScheduleError::ScheduleDeleted { id }.to_string(),
            "schedule 'backup' was deleted"
        );
        assert_eq!(
            RemoveScheduleError::MissingStateValue.to_string(),
            "state value is missing"
        );
        assert_eq!(
            RemoveScheduleError::UnknownStateValue { value: 42 }.to_string(),
            "unknown state value: 42"
        );
    }

    #[test]
    fn rejects_invalid_state_values() {
        assert_eq!(
            current_state(&state_v1::State { state: None }).unwrap_err(),
            RemoveScheduleError::MissingStateValue
        );
        assert_eq!(
            current_state(&state_v1::State {
                state: Some(EnumValue::from(123)),
            })
            .unwrap_err(),
            RemoveScheduleError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            current_state(&state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            })
            .unwrap_err(),
            RemoveScheduleError::UnknownStateValue { value: 0 }
        );
    }

    #[test]
    fn decider_trait_methods_delegate_to_schedule_state() {
        let command = remove_job_command("backup");

        assert_eq!(command.stream_id(), "backup");
        assert_eq!(
            RemoveSchedule::initial_state().state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_MISSING)
        );
        assert_eq!(
            RemoveSchedule::evolve(
                state(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED),
                &removed("backup")
            )
            .unwrap()
            .state
            .unwrap()
            .as_known(),
            Some(state_v1::StateValue::STATE_VALUE_DELETED)
        );
    }
}
