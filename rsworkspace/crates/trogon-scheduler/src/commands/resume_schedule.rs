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
mod tests {
    use buffa::{EnumValue, MessageField};
    use trogon_decider::testing::TestCase;
    use trogon_decider_runtime::CommandSnapshotPolicy;

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

    fn resume_job_command(id: &str) -> ResumeSchedule {
        ResumeSchedule::new(ScheduleId::parse(id).unwrap())
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

    fn resumed(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleResumed {
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

    fn completed(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCompleted {
                    schedule_id: id.to_string(),
                    last_occurrence_sequence: Some(0),
                }
                .into(),
            ),
        }
    }

    #[test]
    fn given_when_then_supports_resume_job_decider() {
        TestCase::<ResumeSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .when(resume_job_command("backup"))
            .then([resumed("backup")]);
    }

    #[test]
    fn given_when_then_supports_resume_job_failures() {
        TestCase::<ResumeSchedule>::new()
            .given([added("backup")])
            .when(resume_job_command("backup"))
            .then_error(ResumeScheduleError::AlreadyActive {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_resuming_completed_jobs() {
        TestCase::<ResumeSchedule>::new()
            .given([added("backup")])
            .given([completed("backup")])
            .given([paused("backup")])
            .when(resume_job_command("backup"))
            .then_error(ResumeScheduleError::AlreadyCompleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_resuming_missing_jobs() {
        TestCase::<ResumeSchedule>::new()
            .given_no_history()
            .when(resume_job_command("backup"))
            .then_error(ResumeScheduleError::ScheduleNotFound {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_resuming_deleted_jobs() {
        TestCase::<ResumeSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .given([removed("backup")])
            .when(resume_job_command("backup"))
            .then_error(ResumeScheduleError::ScheduleDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn errors_display_user_facing_messages() {
        let id = ScheduleId::parse("backup").unwrap();

        assert_eq!(
            ResumeScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
            "schedule 'backup' does not exist"
        );
        assert_eq!(
            ResumeScheduleError::ScheduleDeleted { id: id.clone() }.to_string(),
            "schedule 'backup' was deleted"
        );
        assert_eq!(
            ResumeScheduleError::AlreadyActive { id: id.clone() }.to_string(),
            "schedule 'backup' is already active"
        );
        assert_eq!(
            ResumeScheduleError::AlreadyCompleted { id }.to_string(),
            "schedule 'backup' has already completed its recurrence"
        );
        assert_eq!(
            ResumeScheduleError::MissingStateValue.to_string(),
            "state value is missing"
        );
        assert_eq!(
            ResumeScheduleError::UnknownStateValue { value: 42 }.to_string(),
            "unknown state value: 42"
        );
    }

    #[test]
    fn decide_rejects_invalid_state_values() {
        let command = resume_job_command("backup");

        assert_eq!(
            ResumeSchedule::decide(
                &state_v1::State {
                    completed: None,
                    state: None,
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &command
            )
            .unwrap_err(),
            ResumeScheduleError::MissingStateValue
        );
        assert_eq!(
            ResumeSchedule::decide(
                &state_v1::State {
                    completed: None,
                    state: Some(EnumValue::from(123)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &command,
            )
            .unwrap_err(),
            ResumeScheduleError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            ResumeSchedule::decide(
                &state_v1::State {
                    completed: None,
                    state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &command,
            )
            .unwrap_err(),
            ResumeScheduleError::UnknownStateValue { value: 0 }
        );
    }

    #[test]
    fn command_snapshot_policy_uses_shared_frequency() {
        assert_eq!(
            <ResumeSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY
                .frequency()
                .get(),
            32
        );
    }
}
