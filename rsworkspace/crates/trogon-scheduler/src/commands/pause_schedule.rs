use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
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
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(PauseScheduleError::UnknownStateValue { value: 0 }),
        }
    }
}

impl CommandSnapshotPolicy for PauseSchedule {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use buffa::EnumValue;
    use trogon_decider::testing::TestCase;

    use super::*;
    use crate::commands::domain::{
        Delivery, MessageContent, MessageEnvelope, Schedule as DomainSchedule, ScheduleEventDelivery,
        ScheduleEventSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
    };

    fn schedule_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn pause_job_command(id: &str) -> PauseSchedule {
        PauseSchedule::new(ScheduleId::parse(id).unwrap())
    }

    fn create_schedule(id: &str, status: ScheduleEventStatus) -> crate::CreateSchedule {
        crate::CreateSchedule {
            id: schedule_id(id),
            status,
            schedule: DomainSchedule::every(std::time::Duration::from_secs(30)).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: ScheduleMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: ScheduleHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::ScheduleEvent {
        let command = create_schedule(id, ScheduleEventStatus::Scheduled);

        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: command.id.as_str().to_string(),
                    status: buffa::MessageField::some(v1::ScheduleStatus::from(command.status)),
                    schedule: buffa::MessageField::some(
                        v1::Schedule::try_from(&ScheduleEventSchedule::from(&command.schedule)).unwrap(),
                    ),
                    delivery: buffa::MessageField::some(
                        v1::Delivery::try_from(&ScheduleEventDelivery::from(&command.delivery)).unwrap(),
                    ),
                    message: buffa::MessageField::some(v1::Message::from(&MessageEnvelope::from(&command.message))),
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
        assert_eq!(
            PauseScheduleError::MissingStateValue.to_string(),
            "state value is missing"
        );
        assert_eq!(
            PauseScheduleError::UnknownStateValue { value: 42 }.to_string(),
            "unknown state value: 42"
        );
    }

    #[test]
    fn decide_rejects_invalid_state_values() {
        let command = pause_job_command("backup");

        assert_eq!(
            PauseSchedule::decide(&state_v1::State { state: None }, &command).unwrap_err(),
            PauseScheduleError::MissingStateValue
        );
        assert_eq!(
            PauseSchedule::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(123)),
                },
                &command,
            )
            .unwrap_err(),
            PauseScheduleError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            PauseSchedule::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                },
                &command,
            )
            .unwrap_err(),
            PauseScheduleError::UnknownStateValue { value: 0 }
        );
    }

    #[test]
    fn command_snapshot_policy_uses_shared_frequency() {
        assert_eq!(
            <PauseSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY
                .frequency()
                .get(),
            32
        );
    }
}
